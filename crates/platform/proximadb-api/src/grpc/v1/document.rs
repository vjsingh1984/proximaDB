//! # Document Service (gRPC)
//!
//! gRPC implementation for document storage operations.  Each RPC delegates
//! to the injected `DocumentPort`; when no port is provided the service
//! returns `UNIMPLEMENTED` so the server can start without a document backend.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use proximadb_proto::v1::{
    document_service_server::{DocumentService, DocumentServiceServer},
    *,
};
use proximadb_runtime::DocumentPort;

/// gRPC DocumentService backed by a `DocumentPort`.
pub struct DocumentServiceImpl {
    port: Option<Arc<dyn DocumentPort>>,
}

impl DocumentServiceImpl {
    /// Construct with a concrete document port.
    pub fn new(port: Arc<dyn DocumentPort>) -> Self {
        Self { port: Some(port) }
    }

    /// Construct without a document backend (all RPCs return UNIMPLEMENTED).
    pub fn without_backend() -> Self {
        Self { port: None }
    }

    /// Convert into a tonic gRPC server.
    pub fn into_server(self) -> DocumentServiceServer<Self> {
        DocumentServiceServer::new(self)
    }

    fn not_configured() -> Status {
        Status::unimplemented("Document service not configured on this node")
    }

    fn port_err(e: anyhow::Error) -> Status {
        Status::internal(e.to_string())
    }
}

#[tonic::async_trait]
impl DocumentService for DocumentServiceImpl {
    // ── Collections ───────────────────────────────────────────────────────

    async fn create_collection(
        &self,
        request: Request<CreateDocumentCollectionRequest>,
    ) -> Result<Response<CreateDocumentCollectionResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.create_collection(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn list_collections(
        &self,
        request: Request<ListDocumentCollectionsRequest>,
    ) -> Result<Response<ListDocumentCollectionsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.list_collections(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn delete_collection(
        &self,
        request: Request<DeleteDocumentCollectionRequest>,
    ) -> Result<Response<DeleteDocumentCollectionResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.delete_collection(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Documents ─────────────────────────────────────────────────────────

    async fn insert_document(
        &self,
        request: Request<InsertDocumentRequest>,
    ) -> Result<Response<InsertDocumentResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.insert_document(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn get_document(
        &self,
        request: Request<GetDocumentRequest>,
    ) -> Result<Response<GetDocumentResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.get_document(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn update_document(
        &self,
        request: Request<UpdateDocumentRequest>,
    ) -> Result<Response<UpdateDocumentResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.update_document(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn delete_document(
        &self,
        request: Request<DeleteDocumentRequest>,
    ) -> Result<Response<DeleteDocumentResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.delete_document(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    // ── Queries ───────────────────────────────────────────────────────────

    async fn query_documents(
        &self,
        request: Request<QueryDocumentsRequest>,
    ) -> Result<Response<QueryDocumentsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.query_documents(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }

    async fn aggregate_documents(
        &self,
        request: Request<AggregateDocumentsRequest>,
    ) -> Result<Response<AggregateDocumentsResponse>, Status> {
        let port = self.port.as_ref().ok_or_else(Self::not_configured)?;
        port.aggregate_documents(request.into_inner())
            .await
            .map(Response::new)
            .map_err(Self::port_err)
    }
}
