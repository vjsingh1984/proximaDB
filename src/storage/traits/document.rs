//! Document storage operations trait and related types.
//!
//! This module provides MongoDB-like document storage capabilities
//! using the underlying storage engine with JSON path indexing.

use anyhow::Result;
use async_trait::async_trait;

/// Document storage operations trait (ISP: focused interface for document operations).
///
/// This trait provides MongoDB-like document storage capabilities.
/// Implementations should use the underlying storage engine (SST recommended)
/// with JSON path indexing and document-optimized block formats.
#[async_trait]
pub trait DocumentStorageOperations: Send + Sync {
    /// Insert a document into a collection.
    async fn insert_document(
        &self,
        collection: &str,
        id: &str,
        document: crate::proto::proximadb_v1::SqlObject,
        indexed_paths: Vec<String>,
    ) -> Result<DocumentRecord>;

    /// Get a document by ID.
    async fn get_document(&self, collection: &str, id: &str) -> Result<Option<DocumentRecord>>;

    /// Query documents with filter.
    async fn query_documents(
        &self,
        collection: &str,
        filter: Option<crate::proto::proximadb_v1::DocumentFilter>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<DocumentRecord>>;

    /// Update a document with operations.
    async fn update_document(
        &self,
        collection: &str,
        id: &str,
        updates: Vec<crate::proto::proximadb_v1::DocumentUpdate>,
    ) -> Result<DocumentRecord>;

    /// Delete a document.
    async fn delete_document(&self, collection: &str, id: &str) -> Result<bool>;

    /// Create a document collection with indexes.
    async fn create_document_collection(
        &self,
        config: crate::proto::proximadb_v1::DocumentCollectionConfig,
    ) -> Result<String>;

    /// List document collections.
    async fn list_document_collections(&self) -> Result<Vec<DocumentCollectionInfo>>;
}

/// Document record returned from storage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentRecord {
    pub id: String,
    pub document: crate::proto::proximadb_v1::SqlObject,
    pub version: u64,
    pub created_at_ns: i64,
    pub updated_at_ns: i64,
}

/// Document collection info for listing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentCollectionInfo {
    pub name: String,
    pub document_count: u64,
    pub storage_size_bytes: u64,
    pub indexes: Vec<crate::proto::proximadb_v1::IndexDefinition>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_record() {
        let record = DocumentRecord {
            id: "test".to_string(),
            document: crate::proto::proximadb_v1::SqlObject::default(),
            version: 1,
            created_at_ns: 0,
            updated_at_ns: 0,
        };
        assert_eq!(record.id, "test");
    }
}
