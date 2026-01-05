// Cold tier document retrieval
//
// This module provides abstractions for retrieving documents from cold storage
// (documents that have been flushed to the storage engine but evicted from the hot cache).
//
// Design follows SOLID principles:
// - Single Responsibility: Filter building is separate from search execution
// - Open/Closed: ColdTierRetriever trait allows for different storage backend implementations
// - Dependency Inversion: Uses traits for storage access, not concrete implementations

use anyhow::Result;
use std::sync::Arc;

use crate::core::search::{ComparisonOperator, FilterExpression, SearchParams};
use crate::proto::proximadb_v1::Collection;
use crate::storage::document::DocumentRecord;
use crate::storage::traits::{StorageQueryContext, UnifiedStorageEngine};

// =============================================================================
// TRAIT: ColdTierRetriever (Dependency Inversion Principle)
// =============================================================================

/// Trait for retrieving documents from cold storage tier.
///
/// This trait abstracts the cold tier retrieval mechanism, allowing for
/// different implementations based on the underlying storage backend.
///
/// Implementations can be:
/// - Storage engine based (SST, VIPER, etc.)
/// - External storage (S3, GCS, etc.)
/// - Hybrid approaches
#[async_trait::async_trait]
pub trait ColdTierRetriever: Send + Sync {
    /// Retrieve documents by their IDs from cold storage.
    ///
    /// # Arguments
    /// * `collection` - The collection name/ID
    /// * `ids` - List of document IDs to retrieve
    ///
    /// # Returns
    /// Vector of found documents (may be fewer than requested if some IDs not found)
    async fn retrieve_documents(
        &self,
        collection: &str,
        ids: &[&str],
    ) -> Result<Vec<DocumentRecord>>;

    /// Check if documents exist in cold storage.
    ///
    /// # Arguments
    /// * `collection` - The collection name/ID
    /// * `ids` - List of document IDs to check
    ///
    /// # Returns
    /// Vector of IDs that exist in cold storage
    async fn check_existence(&self, collection: &str, ids: &[&str]) -> Result<Vec<String>>;
}

// =============================================================================
// STRUCT: DocumentMetadataFilterBuilder (Single Responsibility Principle)
// =============================================================================

/// Builder for creating metadata filters to find documents in storage.
///
/// This struct is responsible solely for constructing the filter expressions
/// needed to locate documents in the storage engine.
///
/// Documents are stored with these metadata fields:
/// - `_type`: "document" (type marker)
/// - `_collection`: collection name (routing)
/// - `_document`: serialized JSON document content
/// - `_version`: document version number
pub struct DocumentMetadataFilterBuilder;

impl DocumentMetadataFilterBuilder {
    /// Build a filter expression to find documents by collection and IDs.
    ///
    /// Creates a filter that matches:
    /// - `_type == "document"` AND
    /// - `_collection == collection_name` AND
    /// - `id IN [id1, id2, ...]` (storage IDs are prefixed: "{collection}::{doc_id}")
    ///
    /// # Arguments
    /// * `collection` - The collection name
    /// * `ids` - Document IDs to search for
    ///
    /// # Returns
    /// A FilterExpression that can be used with the storage engine
    pub fn build_document_retrieval_filter(collection: &str, ids: &[&str]) -> FilterExpression {
        // Build the storage-prefixed IDs that documents are stored with
        let storage_ids: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| serde_json::Value::String(format!("{}::{}", collection, id)))
            .collect();

        // Create filter conditions
        let type_filter = FilterExpression::Comparison {
            field: "_type".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String("document".to_string()),
        };

        let collection_filter = FilterExpression::Comparison {
            field: "_collection".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String(collection.to_string()),
        };

        // For ID filtering, we use the vector record ID field (not metadata)
        // The storage engine stores documents with ID format: "{collection}::{doc_id}"
        let id_filter = FilterExpression::Comparison {
            field: "id".to_string(),
            operator: ComparisonOperator::In,
            value: serde_json::Value::Array(storage_ids),
        };

        // Combine all filters with AND
        FilterExpression::And(vec![type_filter, collection_filter, id_filter])
    }

    /// Build a filter expression to find all documents in a collection.
    ///
    /// Creates a filter that matches:
    /// - `_type == "document"` AND
    /// - `_collection == collection_name`
    ///
    /// # Arguments
    /// * `collection` - The collection name
    ///
    /// # Returns
    /// A FilterExpression that can be used with the storage engine
    pub fn build_collection_documents_filter(collection: &str) -> FilterExpression {
        let type_filter = FilterExpression::Comparison {
            field: "_type".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String("document".to_string()),
        };

        let collection_filter = FilterExpression::Comparison {
            field: "_collection".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String(collection.to_string()),
        };

        FilterExpression::And(vec![type_filter, collection_filter])
    }
}

// =============================================================================
// STRUCT: StorageEngineColdTierRetriever (Open/Closed Principle)
// =============================================================================

/// Implementation of ColdTierRetriever using the UnifiedStorageEngine.
///
/// This implementation uses the storage engine's search capability with
/// metadata filtering to retrieve documents from cold storage.
pub struct StorageEngineColdTierRetriever {
    storage_engine: Arc<dyn UnifiedStorageEngine>,
}

impl StorageEngineColdTierRetriever {
    /// Create a new retriever with the given storage engine.
    pub fn new(storage_engine: Arc<dyn UnifiedStorageEngine>) -> Self {
        Self { storage_engine }
    }

    /// Create a minimal Collection config for document retrieval.
    ///
    /// Documents don't require vector operations, so we create a minimal
    /// config with dimension 1 (for the placeholder [0.0] vector).
    fn create_document_collection_config(collection: &str) -> Arc<Collection> {
        use crate::proto::proximadb_v1::{CollectionConfig, StorageAssignment};

        Arc::new(Collection {
            id: format!("_documents_{}", collection),
            config: Some(CollectionConfig {
                name: format!("_documents_{}", collection),
                dimension: 1, // Documents use placeholder vectors
                ..Default::default()
            }),
            storage_assignment: Some(StorageAssignment {
                base_location: "./data".to_string(), // Default location
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    /// Convert an OptimizedSearchRecord to a DocumentRecord.
    ///
    /// Extracts the document data from the metadata fields and reconstructs
    /// the DocumentRecord.
    fn search_record_to_document(
        &self,
        record: &crate::core::search::results::OptimizedSearchRecord,
    ) -> Option<DocumentRecord> {
        use crate::proto::proximadb_v1::sql_value::Value;

        // Check if this is a document record
        let type_value = record.metadata.get("_type")?;
        if let Some(Value::StringValue(t)) = &type_value.value {
            if t != "document" {
                return None;
            }
        } else {
            return None;
        }

        // Get collection
        let collection = record.metadata.get("_collection")?;
        let collection_name = if let Some(Value::StringValue(c)) = &collection.value {
            c.clone()
        } else {
            return None;
        };

        // Get document JSON
        let doc_value = record.metadata.get("_document")?;
        let doc_json = if let Some(Value::StringValue(j)) = &doc_value.value {
            j.clone()
        } else {
            return None;
        };

        // Deserialize document
        let document: crate::proto::proximadb_v1::SqlObject = match serde_json::from_str(&doc_json)
        {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("Failed to deserialize document from storage: {}", e);
                return None;
            }
        };

        // Extract original ID (remove collection prefix)
        let original_id = record
            .id
            .strip_prefix(&format!("{}::", collection_name))
            .unwrap_or(&record.id)
            .to_string();

        // Get version
        let version = record
            .metadata
            .get("_version")
            .and_then(|v| {
                if let Some(Value::Int64Value(i)) = &v.value {
                    Some(*i as u64)
                } else {
                    None
                }
            })
            .unwrap_or(0);

        Some(DocumentRecord {
            id: original_id,
            document,
            collection_id: collection_name,
            version,
            updated_at_ns: record.updated_at.unwrap_or(0) * 1_000_000, // Convert ms to ns
            schema_id: None,
            document_type: None,
        })
    }
}

#[async_trait::async_trait]
impl ColdTierRetriever for StorageEngineColdTierRetriever {
    async fn retrieve_documents(
        &self,
        collection: &str,
        ids: &[&str],
    ) -> Result<Vec<DocumentRecord>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        tracing::debug!(
            "Cold tier: Retrieving {} documents from collection '{}'",
            ids.len(),
            collection
        );

        // Build the filter expression
        let filter = DocumentMetadataFilterBuilder::build_document_retrieval_filter(collection, ids);

        // Create search parameters with the filter
        // We use a placeholder vector since documents don't have real vectors
        let search_params = Arc::new(SearchParams {
            vector: Some(vec![0.0]), // Placeholder vector for document retrieval
            top_k: Some(ids.len()),  // We want at most as many results as IDs requested
            filter_expression: Some(filter),
            include_expired: Some(false),
            ..Default::default()
        });

        // Create collection config
        let collection_config = Self::create_document_collection_config(collection);

        // Create search context
        let ctx = StorageQueryContext::new(search_params, collection_config);

        // Execute search
        let results = self.storage_engine.search_vectors_unified(&ctx).await?;

        tracing::debug!(
            "Cold tier: Retrieved {} results from storage for {} requested IDs",
            results.len(),
            ids.len()
        );

        // Convert search results to documents
        let documents: Vec<DocumentRecord> = results
            .iter()
            .filter_map(|r| self.search_record_to_document(r))
            .collect();

        Ok(documents)
    }

    async fn check_existence(&self, collection: &str, ids: &[&str]) -> Result<Vec<String>> {
        // For existence check, we retrieve documents and return their IDs
        let documents = self.retrieve_documents(collection, ids).await?;
        Ok(documents.into_iter().map(|d| d.id).collect())
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_filter_builder_single_id() {
        let filter =
            DocumentMetadataFilterBuilder::build_document_retrieval_filter("test_collection", &["doc1"]);

        // Should be an AND of 3 conditions
        match filter {
            FilterExpression::And(conditions) => {
                assert_eq!(conditions.len(), 3, "Expected 3 filter conditions");
            }
            _ => panic!("Expected AND filter expression"),
        }
    }

    #[test]
    fn test_document_filter_builder_multiple_ids() {
        let filter = DocumentMetadataFilterBuilder::build_document_retrieval_filter(
            "test_collection",
            &["doc1", "doc2", "doc3"],
        );

        // Verify the filter structure
        match filter {
            FilterExpression::And(conditions) => {
                assert_eq!(conditions.len(), 3, "Expected 3 filter conditions");

                // Find the ID filter
                let id_filter = conditions
                    .iter()
                    .find(|c| matches!(c, FilterExpression::Comparison { field, .. } if field == "id"));

                assert!(id_filter.is_some(), "Expected ID filter");

                if let Some(FilterExpression::Comparison { operator, value, .. }) = id_filter {
                    assert!(
                        matches!(operator, ComparisonOperator::In),
                        "Expected In operator"
                    );

                    if let serde_json::Value::Array(ids) = value {
                        assert_eq!(ids.len(), 3, "Expected 3 IDs in the filter");
                    } else {
                        panic!("Expected array value for In operator");
                    }
                }
            }
            _ => panic!("Expected AND filter expression"),
        }
    }

    #[test]
    fn test_collection_documents_filter() {
        let filter =
            DocumentMetadataFilterBuilder::build_collection_documents_filter("test_collection");

        // Should be an AND of 2 conditions
        match filter {
            FilterExpression::And(conditions) => {
                assert_eq!(conditions.len(), 2, "Expected 2 filter conditions");
            }
            _ => panic!("Expected AND filter expression"),
        }
    }
}
