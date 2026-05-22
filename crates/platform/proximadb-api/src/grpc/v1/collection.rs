//! # Collection Service (gRPC)
//!
//! gRPC implementation for collection management operations.
//! Routes through `ApiHandlersPort` — the seam between this protocol adapter and
//! the business logic in `proximadb-runtime`.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::{self as v1, CollectionOperation};
use proximadb_runtime::ApiHandlersPort;

/// gRPC implementation of the CollectionService for managing vector collections.
///
/// Holds a port reference so the actual business logic can live in the root crate's
/// `UnifiedHandlers` (or any other `ApiHandlersPort` implementation) without this
/// crate importing root-crate concrete types.
pub struct CollectionServiceImpl {
    port: Arc<dyn ApiHandlersPort>,
}

impl CollectionServiceImpl {
    /// Create a new collection service backed by the given port.
    pub fn new(port: Arc<dyn ApiHandlersPort>) -> Self {
        Self { port }
    }

    /// Convert this implementation into a tonic gRPC server.
    pub fn into_server(self) -> v1::collection_service_server::CollectionServiceServer<Self> {
        v1::collection_service_server::CollectionServiceServer::new(self)
    }

    fn extract_tenant_id<T>(request: &Request<T>) -> Option<String> {
        request
            .metadata()
            .get("x-tenant-id")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(ToOwned::to_owned)
    }
}

#[tonic::async_trait]
impl v1::collection_service_server::CollectionService for CollectionServiceImpl {
    async fn create_collection(
        &self,
        request: Request<v1::CollectionConfig>,
    ) -> Result<Response<v1::Collection>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let cfg = request.into_inner();
        let req = v1::CollectionRequest {
            operation: CollectionOperation::CollectionCreate as i32,
            collection_id: Some(cfg.name.clone()),
            collection_config: Some(cfg),
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .port
            .handle_collection_operation_for_tenant(req, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("CreateCollection failed: {e}")))?;
        resp.collection
            .ok_or_else(|| Status::internal("CreateCollection returned no collection"))
            .map(Response::new)
    }

    async fn get_collection(
        &self,
        request: Request<v1::GetCollectionRequest>,
    ) -> Result<Response<v1::Collection>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let inner = request.into_inner();
        let req = v1::CollectionRequest {
            operation: CollectionOperation::CollectionGet as i32,
            collection_id: Some(inner.collection_id),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .port
            .handle_collection_operation_for_tenant(req, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("GetCollection failed: {e}")))?;
        resp.collection
            .ok_or_else(|| Status::not_found("Collection not found"))
            .map(Response::new)
    }

    async fn list_collections(
        &self,
        request: Request<v1::ListCollectionsRequest>,
    ) -> Result<Response<v1::ListCollectionsResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let req = v1::CollectionRequest {
            operation: CollectionOperation::CollectionList as i32,
            collection_id: None,
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        let resp = self
            .port
            .handle_collection_operation_for_tenant(req, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("ListCollections failed: {e}")))?;
        Ok(Response::new(v1::ListCollectionsResponse {
            collections: resp.collections,
        }))
    }

    async fn delete_collection(
        &self,
        request: Request<v1::DeleteCollectionRequest>,
    ) -> Result<Response<v1::DeleteCollectionResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let inner = request.into_inner();
        let req = v1::CollectionRequest {
            operation: CollectionOperation::CollectionDelete as i32,
            collection_id: Some(inner.collection_id),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        self.port
            .handle_collection_operation_for_tenant(req, tenant_id.as_deref())
            .await
            .map_err(|e| Status::internal(format!("DeleteCollection failed: {e}")))?;
        Ok(Response::new(v1::DeleteCollectionResponse {
            success: true,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiCall, RecordingApiPort};
    use tonic::Code;
    use v1::collection_service_server::CollectionService;

    fn collection(name: &str) -> v1::Collection {
        v1::Collection {
            id: format!("{name}-id"),
            config: Some(v1::CollectionConfig {
                name: name.to_string(),
                dimension: 128,
                ..v1::CollectionConfig::default()
            }),
            ..v1::Collection::default()
        }
    }

    fn tenant_request<T>(body: T) -> Request<T> {
        let mut request = Request::new(body);
        request
            .metadata_mut()
            .insert("x-tenant-id", "tenant-a".parse().unwrap());
        request
    }

    #[tokio::test]
    async fn collection_service_lowers_crud_rpcs_to_collection_port_operations() {
        let port = RecordingApiPort::new();
        let docs = collection("docs");
        *port.collection_response.lock().unwrap() = v1::CollectionResponse {
            success: true,
            collection: Some(docs.clone()),
            collections: vec![docs.clone()],
            ..v1::CollectionResponse::default()
        };
        let service = CollectionServiceImpl::new(port.clone());
        let _server = CollectionServiceImpl::new(port.clone()).into_server();

        let created = service
            .create_collection(tenant_request(v1::CollectionConfig {
                name: "docs".to_string(),
                dimension: 128,
                ..v1::CollectionConfig::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(created.id, "docs-id");

        let fetched = service
            .get_collection(tenant_request(v1::GetCollectionRequest {
                collection_id: "docs".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(fetched.id, "docs-id");

        let listed = service
            .list_collections(tenant_request(v1::ListCollectionsRequest::default()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(listed.collections.len(), 1);

        assert!(
            service
                .delete_collection(tenant_request(v1::DeleteCollectionRequest {
                    collection_id: "docs".to_string(),
                }))
                .await
                .unwrap()
                .into_inner()
                .success
        );

        assert_eq!(
            port.calls(),
            vec![
                ApiCall::Collection {
                    operation: CollectionOperation::CollectionCreate as i32,
                    tenant_id: Some("tenant-a".to_string()),
                    collection_id: Some("docs".to_string()),
                },
                ApiCall::Collection {
                    operation: CollectionOperation::CollectionGet as i32,
                    tenant_id: Some("tenant-a".to_string()),
                    collection_id: Some("docs".to_string()),
                },
                ApiCall::Collection {
                    operation: CollectionOperation::CollectionList as i32,
                    tenant_id: Some("tenant-a".to_string()),
                    collection_id: None,
                },
                ApiCall::Collection {
                    operation: CollectionOperation::CollectionDelete as i32,
                    tenant_id: Some("tenant-a".to_string()),
                    collection_id: Some("docs".to_string()),
                },
            ]
        );
    }

    #[tokio::test]
    async fn get_collection_maps_missing_collection_to_not_found() {
        let service = CollectionServiceImpl::new(RecordingApiPort::new());

        let result = service
            .get_collection(Request::new(v1::GetCollectionRequest {
                collection_id: "missing".to_string(),
            }))
            .await;
        let err = match result {
            Ok(_) => panic!("missing collection should map to not_found"),
            Err(err) => err,
        };

        assert_eq!(err.code(), Code::NotFound);
    }
}
