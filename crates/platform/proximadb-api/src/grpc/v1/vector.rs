//! # Vector Service (gRPC)
//!
//! gRPC implementation for vector CRUD and search operations.
//! Routes through `ApiHandlersPort` — the seam between this protocol adapter and
//! the business logic in `proximadb-runtime`.

use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status};

use proximadb_proto::v1::vector_service_server::{VectorService, VectorServiceServer};
use proximadb_proto::v1::{self as v1};
use proximadb_runtime::ApiHandlersPort;

/// gRPC implementation of the VectorService for vector CRUD and search operations.
pub struct VectorServiceImpl {
    port: Arc<dyn ApiHandlersPort>,
}

/// Streaming response type for VectorSearchStream.
pub type VectorSearchStreamStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<v1::SearchVectorRecord, Status>> + Send>>;

impl VectorServiceImpl {
    /// Create a new vector service backed by the given port.
    pub fn new(port: Arc<dyn ApiHandlersPort>) -> Self {
        Self { port }
    }

    /// Convert this implementation into a tonic gRPC server.
    pub fn into_server(self) -> VectorServiceServer<Self> {
        VectorServiceServer::new(self)
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
impl VectorService for VectorServiceImpl {
    async fn vector_batch(
        &self,
        request: Request<v1::VectorBatchRequest>,
    ) -> Result<Response<v1::VectorOperationResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let req = request.into_inner();
        self.port
            .handle_vector_batch_v1_for_tenant(req, tenant_id.as_deref())
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector batch failed: {e}")))
    }

    async fn vector_search(
        &self,
        request: Request<v1::VectorSearchRequest>,
    ) -> Result<Response<v1::VectorOperationResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let req = request.into_inner();
        if tenant_id.is_some() {
            self.port
                .handle_vector_search_v1_for_tenant(req, tenant_id.as_deref())
                .await
                .map(Response::new)
                .map_err(|e| Status::internal(format!("Vector search failed: {e}")))
        } else {
            self.port
                .handle_vector_search_v1(req)
                .await
                .map(Response::new)
                .map_err(|e| Status::internal(format!("Vector search failed: {e}")))
        }
    }

    async fn vector_get(
        &self,
        request: Request<v1::VectorGetRequest>,
    ) -> Result<Response<v1::VectorOperationResponse>, Status> {
        let tenant_id = Self::extract_tenant_id(&request);
        let req = request.into_inner();
        self.port
            .handle_vector_v1_for_tenant(
                &req.collection_id,
                &req.vector_id,
                req.include_vector.unwrap_or(false),
                req.include_metadata.unwrap_or(true),
                tenant_id.as_deref(),
            )
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector get failed: {e}")))
    }

    type VectorSearchStreamStream = VectorSearchStreamStream;

    async fn vector_search_stream(
        &self,
        _request: Request<v1::VectorSearchRequest>,
    ) -> Result<Response<Self::VectorSearchStreamStream>, Status> {
        Err(Status::unimplemented(
            "Streaming vector search is not yet available via this endpoint",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiCall, RecordingApiPort};
    use tonic::Code;

    fn tenant_request<T>(body: T) -> Request<T> {
        let mut request = Request::new(body);
        request
            .metadata_mut()
            .insert("x-tenant-id", "tenant-a".parse().unwrap());
        request
    }

    #[tokio::test]
    async fn vector_service_lowers_search_batch_and_get_to_runtime_port() {
        let port = RecordingApiPort::new();
        port.vector_response.lock().unwrap().success = true;
        let service = VectorServiceImpl::new(port.clone());
        let _server = VectorServiceImpl::new(port.clone()).into_server();

        service
            .vector_search(Request::new(v1::VectorSearchRequest {
                collection_id: "docs".to_string(),
                ..v1::VectorSearchRequest::default()
            }))
            .await
            .unwrap();
        service
            .vector_search(tenant_request(v1::VectorSearchRequest {
                collection_id: "tenant_docs".to_string(),
                ..v1::VectorSearchRequest::default()
            }))
            .await
            .unwrap();
        service
            .vector_batch(tenant_request(v1::VectorBatchRequest {
                collection_id: "docs".to_string(),
                vectors: vec![v1::VectorRecord {
                    id: "vec-1".to_string(),
                    vector: vec![0.1, 0.2],
                    ..v1::VectorRecord::default()
                }],
            }))
            .await
            .unwrap();
        service
            .vector_get(tenant_request(v1::VectorGetRequest {
                collection_id: "docs".to_string(),
                vector_id: "vec-1".to_string(),
                include_vector: Some(false),
                include_metadata: Some(false),
            }))
            .await
            .unwrap();

        assert_eq!(
            port.calls(),
            vec![
                ApiCall::VectorSearch {
                    tenant_id: None,
                    collection_id: "docs".to_string(),
                    tenant_aware: false,
                },
                ApiCall::VectorSearch {
                    tenant_id: Some("tenant-a".to_string()),
                    collection_id: "tenant_docs".to_string(),
                    tenant_aware: true,
                },
                ApiCall::VectorBatch {
                    tenant_id: Some("tenant-a".to_string()),
                    collection_id: "docs".to_string(),
                    vector_count: 1,
                },
                ApiCall::VectorGet {
                    tenant_id: Some("tenant-a".to_string()),
                    collection_id: "docs".to_string(),
                    vector_id: "vec-1".to_string(),
                    include_vector: false,
                    include_metadata: false,
                },
            ]
        );
    }

    #[tokio::test]
    async fn vector_get_defaults_optional_include_flags_and_streaming_is_explicitly_unimplemented()
    {
        let port = RecordingApiPort::new();
        let service = VectorServiceImpl::new(port.clone());

        service
            .vector_get(Request::new(v1::VectorGetRequest {
                collection_id: "docs".to_string(),
                vector_id: "vec-1".to_string(),
                include_vector: None,
                include_metadata: None,
            }))
            .await
            .unwrap();
        assert_eq!(
            port.calls(),
            vec![ApiCall::VectorGet {
                tenant_id: None,
                collection_id: "docs".to_string(),
                vector_id: "vec-1".to_string(),
                include_vector: false,
                include_metadata: true,
            }]
        );

        let result = service
            .vector_search_stream(Request::new(v1::VectorSearchRequest::default()))
            .await;
        let err = match result {
            Ok(_) => panic!("streaming vector search should be unimplemented"),
            Err(err) => err,
        };
        assert_eq!(err.code(), Code::Unimplemented);
        assert!(err.message().contains("Streaming vector search"));
    }
}
