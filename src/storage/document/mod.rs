// Document storage module for MongoDB-like JSON document capabilities
//
// This module provides first-class document storage with:
// - JSON path indexing (B+ tree for range queries)
// - Array indexing (inverted index for containment queries)
// - Full-text search (Tantivy integration)
// - Schema validation and evolution
// - Aggregation pipeline (GROUP BY, COUNT, SUM, AVG, MIN, MAX)
//
// Storage strategy:
// - Hot tier: SST engine with document-optimized blocks
// - Cold tier: VIPER/Parquet columnar storage

pub mod aggregation;
pub mod aggregation_extensions;
pub mod indexes;
pub mod query;
pub mod sdp;
pub mod service;
pub mod storage;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::proto::proximadb_v1::{
    DocumentCollectionConfig, DocumentContent, DocumentFilter, DocumentResult, DocumentUpdate,
    IndexDefinition, SortField, SqlObject,
};

pub use self::service::DocumentService;

// ---------------------------------------------------------------------------
// DocumentStorageEngine trait -- the contract for document-native storage
// ---------------------------------------------------------------------------

/// Storage engine trait for document data model.
///
/// Unlike `UnifiedStorageEngine` (which is vector-centric and returns
/// `OptimizedSearchRecord`), this trait operates on `DocumentRecord` natively.
/// CEDAR implements this trait. DocumentService delegates to it.
#[async_trait]
pub trait DocumentStorageEngine: Send + Sync {
    /// Engine identity
    fn engine_name(&self) -> &'static str;

    /// Insert a document. Returns the stored record with version=1.
    async fn insert_document(
        &self,
        collection: &str,
        doc: DocumentRecord,
    ) -> Result<DocumentRecord>;

    /// Get a document by ID. Returns None if not found.
    async fn get_document(&self, collection: &str, id: &str) -> Result<Option<DocumentRecord>>;

    /// Update a document by ID. Returns the updated record with incremented version.
    async fn update_document(
        &self,
        collection: &str,
        id: &str,
        updates: Vec<DocumentUpdate>,
    ) -> Result<DocumentRecord>;

    /// Delete a document by ID. Returns true if the document existed.
    async fn delete_document(&self, collection: &str, id: &str) -> Result<bool>;

    /// Query documents with filters, projection, sort, and pagination.
    async fn query_documents(
        &self,
        collection: &str,
        params: DocumentQueryParams,
    ) -> Result<DocumentQueryResult>;

    /// Scan all documents in a collection (with optional limit).
    async fn scan_documents(
        &self,
        collection: &str,
        limit: Option<usize>,
    ) -> Result<Vec<DocumentRecord>>;

    /// Run an aggregation pipeline.
    async fn aggregate(
        &self,
        collection: &str,
        pipeline: Vec<crate::proto::proximadb_v1::AggregationStage>,
    ) -> Result<AggregateResult>;

    /// Create a secondary index on a field.
    async fn create_index(&self, collection: &str, index_def: IndexDefinition) -> Result<()>;

    /// Flush in-memory data to persistent storage.
    async fn flush(&self, collection: &str) -> Result<FlushToStorageResult>;

    /// Compact on-disk files (merge, deduplicate, reclaim space).
    async fn compact(&self, collection: &str) -> Result<FlushToStorageResult>;

    /// Get document count for a collection.
    async fn document_count(&self, collection: &str) -> Result<u64>;

    /// Collect engine-specific metrics.
    async fn collect_metrics(&self) -> Result<HashMap<String, serde_json::Value>>;
}

/// Document record with ID, version, and content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentRecord {
    /// Unique document ID
    pub id: String,
    /// Document content as nested JSON
    pub document: SqlObject,
    /// Version number for optimistic locking
    pub version: u64,
    /// Collection this document belongs to
    pub collection_id: String,
    /// Timestamp of last modification (nanoseconds)
    pub updated_at_ns: i64,
    /// Optional schema ID for validation
    pub schema_id: Option<String>,
    /// Document type/kind for type-based queries
    pub document_type: Option<String>,
}

impl DocumentRecord {
    /// Create a new document record
    pub fn new(id: String, document: SqlObject, collection_id: String) -> Self {
        Self {
            id,
            document,
            version: 1,
            collection_id,
            updated_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            schema_id: None,
            document_type: None,
        }
    }

    /// Create from proto DocumentContent
    pub fn from_proto(id: String, content: DocumentContent, collection_id: String) -> Result<Self> {
        Ok(Self {
            id,
            document: content.document.unwrap_or_default(),
            version: 1,
            collection_id,
            updated_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            schema_id: content.schema_id,
            document_type: content.document_type,
        })
    }

    /// Convert to proto DocumentContent
    pub fn to_proto_content(&self) -> DocumentContent {
        self.to_proto_content_with_paths(&[])
    }

    /// Convert to proto DocumentContent with indexed paths
    pub fn to_proto_content_with_paths(&self, indexed_paths: &[String]) -> DocumentContent {
        DocumentContent {
            document: Some(self.document.clone()),
            schema_id: self.schema_id.clone(),
            indexed_paths: indexed_paths.to_vec(),
            document_type: self.document_type.clone(),
        }
    }

    /// Convert to proto DocumentContent with collection config
    pub fn to_proto_content_from_config(
        &self,
        config: &DocumentCollectionConfig,
    ) -> DocumentContent {
        let indexed_paths: Vec<String> =
            config.indexes.iter().map(|idx| idx.path.clone()).collect();
        self.to_proto_content_with_paths(&indexed_paths)
    }

    /// Convert to proto DocumentResult
    pub fn to_proto_result(&self, score: Option<f32>) -> DocumentResult {
        DocumentResult {
            id: self.id.clone(),
            document: Some(self.document.clone()),
            version: self.version,
            score,
        }
    }
}

/// Document collection metadata
#[derive(Debug, Clone)]
pub struct DocumentCollection {
    /// Collection name
    pub name: String,
    /// Collection configuration
    pub config: DocumentCollectionConfig,
    /// Index definitions
    pub indexes: Vec<IndexDefinition>,
    /// Number of documents
    pub document_count: u64,
    /// Storage size in bytes
    pub storage_size_bytes: u64,
    /// Created timestamp (nanoseconds)
    pub created_at_ns: i64,
    /// Updated timestamp (nanoseconds)
    pub updated_at_ns: i64,
}

impl DocumentCollection {
    /// Create a new document collection
    pub fn new(name: String, config: DocumentCollectionConfig) -> Self {
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let indexes = config.indexes.clone();
        Self {
            name,
            config,
            indexes,
            document_count: 0,
            storage_size_bytes: 0,
            created_at_ns: now,
            updated_at_ns: now,
        }
    }
}

/// Query parameters for document search
#[derive(Debug, Clone, Default)]
pub struct DocumentQueryParams {
    /// Filter conditions
    pub filter: Option<DocumentFilter>,
    /// Fields to project (empty = all fields)
    pub projection: Vec<String>,
    /// Sort fields
    pub sort: Vec<SortField>,
    /// Maximum results
    pub limit: u32,
    /// Offset for pagination
    pub offset: u32,
    /// Include total count (slower query)
    pub include_count: bool,
}

/// Result of a document query
#[derive(Debug, Clone)]
pub struct DocumentQueryResult {
    /// Matched documents
    pub documents: Vec<DocumentRecord>,
    /// Total count (if requested)
    pub total_count: Option<u64>,
    /// Query execution time in milliseconds
    pub query_time_ms: u64,
}

/// Ingest result for bulk operations
#[derive(Debug, Clone, Default)]
pub struct DocumentIngestResult {
    /// Number of documents successfully ingested
    pub ingested: u64,
    /// Number of documents that failed
    pub failed: u64,
    /// Error messages for failed documents
    pub errors: Vec<String>,
    /// Processing time in milliseconds
    pub processing_time_ms: u64,
}

/// Result of flushing documents to storage engine
#[derive(Debug, Clone, Default)]
pub struct FlushToStorageResult {
    /// Number of documents flushed
    pub documents_flushed: usize,
    /// Bytes written to storage
    pub bytes_written: usize,
    /// Duration of the flush operation in milliseconds
    pub duration_ms: u64,
    /// Whether the operation was successful
    pub success: bool,
}

/// Result of a document aggregation pipeline
#[derive(Debug, Clone)]
pub struct AggregateResult {
    /// Aggregated result documents (output of the pipeline)
    pub results: Vec<SqlObject>,
    /// Query execution time in milliseconds
    pub query_time_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::{DocIndexType, IndexDefinition};

    #[test]
    fn test_document_record_new() {
        let doc = DocumentRecord::new(
            "doc1".to_string(),
            SqlObject::default(),
            "test_collection".to_string(),
        );
        assert_eq!(doc.id, "doc1");
        assert_eq!(doc.version, 1);
        assert_eq!(doc.collection_id, "test_collection");
    }

    #[test]
    fn test_document_collection_new() {
        let config = DocumentCollectionConfig {
            name: "test".to_string(),
            ..Default::default()
        };
        let collection = DocumentCollection::new("test".to_string(), config);
        assert_eq!(collection.name, "test");
        assert_eq!(collection.document_count, 0);
    }

    #[test]
    fn test_to_proto_content_default() {
        let doc = DocumentRecord::new(
            "doc1".to_string(),
            SqlObject::default(),
            "test_collection".to_string(),
        );
        let content = doc.to_proto_content();
        assert!(content.document.is_some());
        assert!(content.indexed_paths.is_empty());
        assert!(content.schema_id.is_none());
    }

    #[test]
    fn test_to_proto_content_with_paths() {
        let doc = DocumentRecord::new(
            "doc1".to_string(),
            SqlObject::default(),
            "test_collection".to_string(),
        );
        let paths = vec!["$.user.email".to_string(), "$.user.name".to_string()];
        let content = doc.to_proto_content_with_paths(&paths);
        assert_eq!(content.indexed_paths, paths);
    }

    #[test]
    fn test_to_proto_content_from_config() {
        let doc = DocumentRecord::new(
            "doc1".to_string(),
            SqlObject::default(),
            "test_collection".to_string(),
        );

        let mut config = DocumentCollectionConfig {
            name: "test".to_string(),
            ..Default::default()
        };

        config.indexes = vec![
            IndexDefinition {
                path: "$.user.email".to_string(),
                index_type: DocIndexType::Btree as i32,
                unique: true,
                ..Default::default()
            },
            IndexDefinition {
                path: "$.user.name".to_string(),
                index_type: DocIndexType::Hash as i32,
                unique: false,
                ..Default::default()
            },
        ];

        let content = doc.to_proto_content_from_config(&config);
        assert_eq!(content.indexed_paths.len(), 2);
        assert_eq!(content.indexed_paths[0], "$.user.email");
        assert_eq!(content.indexed_paths[1], "$.user.name");
    }

    #[test]
    fn test_from_proto() {
        let proto_content = DocumentContent {
            document: Some(SqlObject::default()),
            schema_id: Some("schema123".to_string()),
            indexed_paths: vec!["$.field1".to_string()],
            document_type: Some("user".to_string()),
        };

        let doc = DocumentRecord::from_proto(
            "doc1".to_string(),
            proto_content,
            "test_collection".to_string(),
        )
        .unwrap();

        assert_eq!(doc.id, "doc1");
        assert_eq!(doc.version, 1);
        assert_eq!(doc.collection_id, "test_collection");
        assert_eq!(doc.schema_id, Some("schema123".to_string()));
        assert_eq!(doc.document_type, Some("user".to_string()));
    }
}
