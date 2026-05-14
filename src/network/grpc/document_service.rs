// DEPRECATED: This file has been migrated to crates/platform/proximadb-api/src/grpc/v1/document.rs
// Please use: use proximadb_api::grpc::DocumentServiceImpl;
// This compatibility shim will be removed in version 0.3.0

// Document gRPC service implementation
//
// Implements the DocumentService defined in proto/proximadb/v1/document.proto

use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::proto::proximadb_v1;
use crate::proto::proximadb_v1::document_service_server::{DocumentService, DocumentServiceServer};
use crate::storage::document::{DocumentQueryParams, DocumentService as DocStorageService};

/// Document gRPC service implementation
pub struct DocumentServiceImpl {
    document_service: Arc<DocStorageService>,
}

impl DocumentServiceImpl {
    /// Create a new document service with the given storage service
    pub fn new(document_service: Arc<DocStorageService>) -> Self {
        Self { document_service }
    }

    /// Convert to tonic server
    pub fn into_server(self) -> DocumentServiceServer<Self> {
        DocumentServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl DocumentService for DocumentServiceImpl {
    async fn create_collection(
        &self,
        request: Request<proximadb_v1::CreateDocumentCollectionRequest>,
    ) -> Result<Response<proximadb_v1::CreateDocumentCollectionResponse>, Status> {
        let req = request.into_inner();
        let config = req
            .config
            .ok_or_else(|| Status::invalid_argument("Missing config"))?;
        let name = config.name.clone();

        match self.document_service.create_collection(&name, config).await {
            Ok(id) => Ok(Response::new(
                proximadb_v1::CreateDocumentCollectionResponse {
                    collection_id: id,
                    success: true,
                },
            )),
            Err(e) => Err(Status::internal(format!(
                "Failed to create collection: {}",
                e
            ))),
        }
    }

    async fn list_collections(
        &self,
        _request: Request<proximadb_v1::ListDocumentCollectionsRequest>,
    ) -> Result<Response<proximadb_v1::ListDocumentCollectionsResponse>, Status> {
        match self.document_service.list_collections().await {
            Ok(collections) => {
                let infos: Vec<proximadb_v1::DocumentCollectionInfo> = collections
                    .iter()
                    .map(|c| proximadb_v1::DocumentCollectionInfo {
                        name: c.name.clone(),
                        document_count: c.document_count,
                        storage_size_bytes: c.storage_size_bytes,
                        indexes: c.indexes.clone(),
                    })
                    .collect();
                Ok(Response::new(
                    proximadb_v1::ListDocumentCollectionsResponse { collections: infos },
                ))
            }
            Err(e) => Err(Status::internal(format!(
                "Failed to list collections: {}",
                e
            ))),
        }
    }

    async fn delete_collection(
        &self,
        request: Request<proximadb_v1::DeleteDocumentCollectionRequest>,
    ) -> Result<Response<proximadb_v1::DeleteDocumentCollectionResponse>, Status> {
        let req = request.into_inner();
        match self
            .document_service
            .delete_collection(&req.collection)
            .await
        {
            Ok(_) => Ok(Response::new(
                proximadb_v1::DeleteDocumentCollectionResponse { success: true },
            )),
            Err(e) => Err(Status::internal(format!(
                "Failed to delete collection: {}",
                e
            ))),
        }
    }

    async fn insert_document(
        &self,
        request: Request<proximadb_v1::InsertDocumentRequest>,
    ) -> Result<Response<proximadb_v1::InsertDocumentResponse>, Status> {
        let req = request.into_inner();
        let document = req
            .document
            .ok_or_else(|| Status::invalid_argument("Missing document"))?;
        let id = req.id.as_deref();

        match self
            .document_service
            .insert_document(&req.collection, id, document)
            .await
        {
            Ok(record) => Ok(Response::new(proximadb_v1::InsertDocumentResponse {
                id: record.id,
                version: record.version,
            })),
            Err(e) => Err(Status::internal(format!(
                "Failed to insert document: {}",
                e
            ))),
        }
    }

    async fn get_document(
        &self,
        request: Request<proximadb_v1::GetDocumentRequest>,
    ) -> Result<Response<proximadb_v1::GetDocumentResponse>, Status> {
        let req = request.into_inner();
        let projection = if req.projection.is_empty() {
            None
        } else {
            Some(req.projection)
        };

        match self
            .document_service
            .get_document(&req.collection, &req.id, projection)
            .await
        {
            Ok(Some(record)) => Ok(Response::new(proximadb_v1::GetDocumentResponse {
                document: Some(record.document),
                version: record.version,
                found: true,
            })),
            Ok(None) => Ok(Response::new(proximadb_v1::GetDocumentResponse {
                document: None,
                version: 0,
                found: false,
            })),
            Err(e) => Err(Status::internal(format!("Failed to get document: {}", e))),
        }
    }

    async fn update_document(
        &self,
        request: Request<proximadb_v1::UpdateDocumentRequest>,
    ) -> Result<Response<proximadb_v1::UpdateDocumentResponse>, Status> {
        let req = request.into_inner();

        match self
            .document_service
            .update_document(&req.collection, &req.id, req.updates, req.expected_version)
            .await
        {
            Ok(record) => Ok(Response::new(proximadb_v1::UpdateDocumentResponse {
                new_version: record.version,
                success: true,
            })),
            Err(e) => Err(Status::internal(format!(
                "Failed to update document: {}",
                e
            ))),
        }
    }

    async fn delete_document(
        &self,
        request: Request<proximadb_v1::DeleteDocumentRequest>,
    ) -> Result<Response<proximadb_v1::DeleteDocumentResponse>, Status> {
        let req = request.into_inner();

        match self
            .document_service
            .delete_document(&req.collection, &req.id)
            .await
        {
            Ok(deleted) => Ok(Response::new(proximadb_v1::DeleteDocumentResponse {
                deleted,
            })),
            Err(e) => Err(Status::internal(format!(
                "Failed to delete document: {}",
                e
            ))),
        }
    }

    async fn query_documents(
        &self,
        request: Request<proximadb_v1::QueryDocumentsRequest>,
    ) -> Result<Response<proximadb_v1::QueryDocumentsResponse>, Status> {
        let req = request.into_inner();

        let params = DocumentQueryParams {
            filter: req.filter,
            projection: req.projection,
            sort: req.sort,
            limit: req.limit,
            offset: req.offset,
            include_count: req.include_count,
        };

        match self
            .document_service
            .query_documents(&req.collection, params)
            .await
        {
            Ok(result) => {
                let documents: Vec<proximadb_v1::DocumentResult> = result
                    .documents
                    .into_iter()
                    .map(|d| proximadb_v1::DocumentResult {
                        id: d.id,
                        document: Some(d.document),
                        version: d.version,
                        score: None,
                    })
                    .collect();

                Ok(Response::new(proximadb_v1::QueryDocumentsResponse {
                    documents,
                    total_count: result.total_count,
                    query_time_ms: result.query_time_ms,
                }))
            }
            Err(e) => Err(Status::internal(format!(
                "Failed to query documents: {}",
                e
            ))),
        }
    }

    async fn aggregate_documents(
        &self,
        request: Request<proximadb_v1::AggregateDocumentsRequest>,
    ) -> Result<Response<proximadb_v1::AggregateDocumentsResponse>, Status> {
        let req = request.into_inner();

        match self
            .document_service
            .aggregate_documents(&req.collection, req.filter, req.pipeline)
            .await
        {
            Ok(result) => Ok(Response::new(proximadb_v1::AggregateDocumentsResponse {
                results: result.results,
                query_time_ms: result.query_time_ms,
            })),
            Err(e) => Err(Status::internal(format!(
                "Failed to aggregate documents: {}",
                e
            ))),
        }
    }
}

// ── DocumentPort ──────────────────────────────────────────────────────────────

use proximadb_v1::{
    AggregateDocumentsRequest, AggregateDocumentsResponse, CreateDocumentCollectionRequest,
    CreateDocumentCollectionResponse, DeleteDocumentCollectionRequest,
    DeleteDocumentCollectionResponse, DeleteDocumentRequest, DeleteDocumentResponse,
    GetDocumentRequest, GetDocumentResponse, InsertDocumentRequest, InsertDocumentResponse,
    ListDocumentCollectionsRequest, ListDocumentCollectionsResponse, QueryDocumentsRequest,
    QueryDocumentsResponse, UpdateDocumentRequest, UpdateDocumentResponse,
};

#[async_trait::async_trait]
impl proximadb_runtime::DocumentPort for DocumentServiceImpl {
    async fn create_collection(
        &self,
        request: CreateDocumentCollectionRequest,
    ) -> anyhow::Result<CreateDocumentCollectionResponse> {
        DocumentService::create_collection(self, Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn list_collections(
        &self,
        request: ListDocumentCollectionsRequest,
    ) -> anyhow::Result<ListDocumentCollectionsResponse> {
        DocumentService::list_collections(self, Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn delete_collection(
        &self,
        request: DeleteDocumentCollectionRequest,
    ) -> anyhow::Result<DeleteDocumentCollectionResponse> {
        DocumentService::delete_collection(self, Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn insert_document(
        &self,
        request: InsertDocumentRequest,
    ) -> anyhow::Result<InsertDocumentResponse> {
        DocumentService::insert_document(self, Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn get_document(
        &self,
        request: GetDocumentRequest,
    ) -> anyhow::Result<GetDocumentResponse> {
        DocumentService::get_document(self, Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn update_document(
        &self,
        request: UpdateDocumentRequest,
    ) -> anyhow::Result<UpdateDocumentResponse> {
        DocumentService::update_document(self, Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn delete_document(
        &self,
        request: DeleteDocumentRequest,
    ) -> anyhow::Result<DeleteDocumentResponse> {
        DocumentService::delete_document(self, Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn query_documents(
        &self,
        request: QueryDocumentsRequest,
    ) -> anyhow::Result<QueryDocumentsResponse> {
        DocumentService::query_documents(self, Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }

    async fn aggregate_documents(
        &self,
        request: AggregateDocumentsRequest,
    ) -> anyhow::Result<AggregateDocumentsResponse> {
        DocumentService::aggregate_documents(self, Request::new(request))
            .await
            .map(|r| r.into_inner())
            .map_err(|s| anyhow::anyhow!("{}", s.message()))
    }
}
