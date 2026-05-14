//! # Document Service (gRPC)
//!
//! gRPC implementation for document storage operations.
//!
//! ## Status
//!
//! **TEMPORARY PLACEHOLDER**: This module contains placeholder implementations during the
//! workspace refactor. The actual implementations exist in `src/network/grpc/document_service.rs`.

use std::sync::Arc;
use tonic::{Request, Response, Status};

// Placeholder types for document services
// TODO: Replace with actual types after migration
pub struct DocStorageService;

use proximadb_proto::v1;
use proximadb_proto::v1::document_service_server::{DocumentService, DocumentServiceServer};

/// Document gRPC service implementation
pub struct DocumentServiceImpl {
    _document_service: Arc<DocStorageService>,
}

impl DocumentServiceImpl {
    /// Create a new document service with the given storage service
    pub fn new(_document_service: Arc<DocStorageService>) -> Self {
        Self { _document_service }
    }

    /// Convert to tonic server
    pub fn into_server(self) -> DocumentServiceServer<Self> {
        DocumentServiceServer::new(self)
    }
}

// Placeholder trait implementation - will be implemented after migration
#[tonic::async_trait]
impl DocumentService for DocumentServiceImpl {
    async fn create_collection(
        &self,
        _request: Request<v1::CreateDocumentCollectionRequest>,
    ) -> Result<Response<v1::CreateDocumentCollectionResponse>, Status> {
        Err(Status::unimplemented("Document service migration in progress"))
    }

    async fn list_collections(
        &self,
        _request: Request<v1::ListDocumentCollectionsRequest>,
    ) -> Result<Response<v1::ListDocumentCollectionsResponse>, Status> {
        Err(Status::unimplemented("Document service migration in progress"))
    }

    async fn delete_collection(
        &self,
        _request: Request<v1::DeleteDocumentCollectionRequest>,
    ) -> Result<Response<v1::DeleteDocumentCollectionResponse>, Status> {
        Err(Status::unimplemented("Document service migration in progress"))
    }

    async fn insert_document(
        &self,
        _request: Request<v1::InsertDocumentRequest>,
    ) -> Result<Response<v1::InsertDocumentResponse>, Status> {
        Err(Status::unimplemented("Document service migration in progress"))
    }

    async fn get_document(
        &self,
        _request: Request<v1::GetDocumentRequest>,
    ) -> Result<Response<v1::GetDocumentResponse>, Status> {
        Err(Status::unimplemented("Document service migration in progress"))
    }

    async fn update_document(
        &self,
        _request: Request<v1::UpdateDocumentRequest>,
    ) -> Result<Response<v1::UpdateDocumentResponse>, Status> {
        Err(Status::unimplemented("Document service migration in progress"))
    }

    async fn delete_document(
        &self,
        _request: Request<v1::DeleteDocumentRequest>,
    ) -> Result<Response<v1::DeleteDocumentResponse>, Status> {
        Err(Status::unimplemented("Document service migration in progress"))
    }

    async fn query_documents(
        &self,
        _request: Request<v1::QueryDocumentsRequest>,
    ) -> Result<Response<v1::QueryDocumentsResponse>, Status> {
        Err(Status::unimplemented("Document service migration in progress"))
    }

    async fn aggregate_documents(
        &self,
        _request: Request<v1::AggregateDocumentsRequest>,
    ) -> Result<Response<v1::AggregateDocumentsResponse>, Status> {
        Err(Status::unimplemented("Document service migration in progress"))
    }
}
