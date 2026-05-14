//! Document storage composition port trait for `proximadb-runtime`.
//!
//! `DocumentPort` is the stable contract that the gRPC `DocumentService`
//! in `proximadb-api` uses to call into the document subsystem without
//! importing root-crate concrete types.
//!
//! Every method maps to a gRPC `DocumentService` RPC.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_proto::v1::{
    AggregateDocumentsRequest, AggregateDocumentsResponse, CreateDocumentCollectionRequest,
    CreateDocumentCollectionResponse, DeleteDocumentCollectionRequest,
    DeleteDocumentCollectionResponse, DeleteDocumentRequest, DeleteDocumentResponse,
    GetDocumentRequest, GetDocumentResponse, InsertDocumentRequest, InsertDocumentResponse,
    ListDocumentCollectionsRequest, ListDocumentCollectionsResponse, QueryDocumentsRequest,
    QueryDocumentsResponse, UpdateDocumentRequest, UpdateDocumentResponse,
};

/// Port for document storage operations.
///
/// Implemented by root-crate `DocumentServiceImpl`.  When absent, the gRPC
/// adapter returns `UNIMPLEMENTED` so the server can start without a document
/// backend configured.
#[async_trait]
pub trait DocumentPort: Send + Sync {
    // ── Collections ───────────────────────────────────────────────────────

    async fn create_collection(
        &self,
        request: CreateDocumentCollectionRequest,
    ) -> Result<CreateDocumentCollectionResponse>;

    async fn list_collections(
        &self,
        request: ListDocumentCollectionsRequest,
    ) -> Result<ListDocumentCollectionsResponse>;

    async fn delete_collection(
        &self,
        request: DeleteDocumentCollectionRequest,
    ) -> Result<DeleteDocumentCollectionResponse>;

    // ── Documents ─────────────────────────────────────────────────────────

    async fn insert_document(
        &self,
        request: InsertDocumentRequest,
    ) -> Result<InsertDocumentResponse>;

    async fn get_document(
        &self,
        request: GetDocumentRequest,
    ) -> Result<GetDocumentResponse>;

    async fn update_document(
        &self,
        request: UpdateDocumentRequest,
    ) -> Result<UpdateDocumentResponse>;

    async fn delete_document(
        &self,
        request: DeleteDocumentRequest,
    ) -> Result<DeleteDocumentResponse>;

    // ── Queries ───────────────────────────────────────────────────────────

    async fn query_documents(
        &self,
        request: QueryDocumentsRequest,
    ) -> Result<QueryDocumentsResponse>;

    async fn aggregate_documents(
        &self,
        request: AggregateDocumentsRequest,
    ) -> Result<AggregateDocumentsResponse>;
}
