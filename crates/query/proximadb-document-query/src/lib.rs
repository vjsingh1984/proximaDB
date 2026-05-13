// ============================================================================
// Document Service Contract (Phase 2.2 - TDD Implementation)
// ============================================================================

use async_trait::async_trait;
use proximadb_kernel::error::ProximaDBError;
use proximadb_proto::proximadb_v1::DocumentResult;
use proximadb_query_filter::{FilterOperator, FilterValue};

/// Stable document record shape exposed by the document-query contract.
pub type DocumentRecord = DocumentResult;

/// Canonical document-query contract result type.
pub type DocumentQueryResult<T> = std::result::Result<T, ProximaDBError>;

/// Core document search request for stable cross-modal queries.
///
/// This request type uses proto-backed types (`DocumentRecord`) for results and stable
/// primitive types for parameters, making it suitable for trait-based service
/// contracts without depending on legacy request types.
#[derive(Debug, Clone)]
pub struct DocumentSearchRequest {
    /// Collection to search.
    pub collection_id: String,
    /// Filter expression for metadata filtering.
    pub filter: Option<String>,
    /// Number of results to return.
    pub limit: usize,
    /// Offset for pagination.
    pub offset: usize,
    /// Projection - which fields to return (None = all fields).
    pub projection: Option<Vec<String>>,
    /// Sort order.
    pub sort: Option<DocumentSortOrder>,
}

/// Document sort order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSortOrder {
    /// Field to sort by.
    pub field: String,
    /// Sort direction.
    pub direction: SortDirection,
}

/// Sort direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortDirection {
    /// Ascending order.
    Ascending,
    /// Descending order.
    Descending,
}

/// Document search result using proto types for stability.
#[derive(Debug, Clone)]
pub struct DocumentSearchResult {
    /// Retrieved documents.
    pub results: Vec<DocumentRecord>,
    /// Total count before limit/offset was applied.
    pub total_count: usize,
    /// Query execution time in milliseconds.
    pub execution_time_ms: u64,
}

/// Narrow async document-query contract for document-facing query runtimes.
///
/// This trait defines the core document search operations that cross-modal query
/// orchestration depends on. It is intentionally narrow, focusing on read/query
/// operations. Write operations (insert, delete) are handled separately to allow
/// for different permission and consistency models.
///
/// Design principles:
/// - **Narrow**: Only essential search operations to keep the trait focused
/// - **Stable types**: Uses proto-backed types (`DocumentRecord`) for results
/// - **Async**: All operations are async to support multiple storage backends
/// - **Error handling**: Uses `ProximaDBError` for consistent error reporting
#[async_trait]
pub trait DocumentQueryService: Send + Sync {
    /// Execute a document search.
    ///
    /// # Arguments
    ///
    /// * `request` - Document search parameters including collection, filter, limit, offset
    ///
    /// # Returns
    ///
    /// * `DocumentSearchResult` - Search results with documents, metadata, and timing
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let request = DocumentSearchRequest {
    ///     collection_id: "documents".to_string(),
    ///     filter: Some("category = 'tech'".to_string()),
    ///     limit: 10,
    ///     offset: 0,
    ///     projection: None,
    ///     sort: None,
    /// };
    /// let result = service.document_search(request).await?;
    /// ```
    async fn document_search(
        &self,
        request: DocumentSearchRequest,
    ) -> DocumentQueryResult<DocumentSearchResult>;

    /// Get a document by ID.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - Collection containing the document
    /// * `document_id` - Unique document identifier
    ///
    /// # Returns
    ///
    /// * `Option<DocumentRecord>` - The document if found, None otherwise
    async fn get_document(
        &self,
        collection_id: String,
        document_id: String,
    ) -> DocumentQueryResult<Option<DocumentRecord>>;
}

// ============================================================================
// Legacy Expression Types (kept for backward compatibility)
// ============================================================================

/// Document query expression used by cross-model query orchestration.
#[derive(Debug, Clone)]
pub struct DocumentQueryExpr {
    /// Collection to query.
    pub collection: String,
    /// JSON path filters.
    pub path_filters: Vec<PathFilter>,
    /// Full-text search query.
    pub text_search: Option<String>,
    /// Projection (fields to return).
    pub projection: Vec<String>,
    /// Sort order.
    pub sort: Option<DocumentSort>,
    /// Limit.
    pub limit: Option<u32>,
}

/// JSON path filter used by document queries.
#[derive(Debug, Clone)]
pub struct PathFilter {
    /// JSON path (e.g. `"$.user.name"`).
    pub path: String,
    /// Comparison operator.
    pub operator: FilterOperator,
    /// Value to compare against.
    pub value: FilterValue,
}

/// Document sort specification.
#[derive(Debug, Clone)]
pub struct DocumentSort {
    /// Path to sort by.
    pub path: String,
    /// Ascending or descending.
    pub ascending: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_query_expr_carries_filter_projection_and_limit() {
        let expr = DocumentQueryExpr {
            collection: "products".to_string(),
            path_filters: vec![PathFilter {
                path: "$.category".to_string(),
                operator: FilterOperator::Eq,
                value: FilterValue::String("electronics".to_string()),
            }],
            text_search: Some("laptop".to_string()),
            projection: vec!["id".to_string(), "name".to_string()],
            sort: Some(DocumentSort {
                path: "$.price".to_string(),
                ascending: true,
            }),
            limit: Some(25),
        };

        assert_eq!(expr.collection, "products");
        assert_eq!(expr.path_filters.len(), 1);
        assert_eq!(expr.projection, vec!["id", "name"]);
        assert_eq!(expr.limit, Some(25));
        assert!(expr.text_search.is_some());
    }

    #[test]
    fn document_sort_direction_is_explicit() {
        let ascending = DocumentSort {
            path: "$.created_at".to_string(),
            ascending: true,
        };
        let descending = DocumentSort {
            path: "$.created_at".to_string(),
            ascending: false,
        };

        assert!(ascending.ascending);
        assert!(!descending.ascending);
    }

    // ========================================================================
    // DocumentQueryService Trait Tests (TDD)
    // ========================================================================

    #[test]
    fn document_search_request_has_required_fields() {
        let request = DocumentSearchRequest {
            collection_id: "test_collection".to_string(),
            filter: Some("status = 'active'".to_string()),
            limit: 10,
            offset: 0,
            projection: Some(vec!["id".to_string(), "title".to_string()]),
            sort: Some(DocumentSortOrder {
                field: "created_at".to_string(),
                direction: SortDirection::Descending,
            }),
        };

        assert_eq!(request.collection_id, "test_collection");
        assert_eq!(request.filter.as_deref(), Some("status = 'active'"));
        assert_eq!(request.limit, 10);
        assert_eq!(request.offset, 0);
        assert_eq!(request.projection.as_ref().map(|v| v.len()), Some(2));
        assert_eq!(
            request.sort.as_ref().map(|s| &s.field),
            Some(&"created_at".to_string())
        );
    }

    #[test]
    fn document_search_request_defaults() {
        let request = DocumentSearchRequest {
            collection_id: "test".to_string(),
            filter: None,
            limit: 5,
            offset: 0,
            projection: None,
            sort: None,
        };

        assert_eq!(request.collection_id, "test");
        assert!(request.filter.is_none());
        assert_eq!(request.limit, 5);
        assert!(request.projection.is_none());
        assert!(request.sort.is_none());
    }

    #[test]
    fn document_search_result_contains_results_and_metadata() {
        let result = DocumentSearchResult {
            results: vec![],
            total_count: 0,
            execution_time_ms: 100,
        };

        assert_eq!(result.results.len(), 0);
        assert_eq!(result.total_count, 0);
        assert_eq!(result.execution_time_ms, 100);
    }

    #[test]
    fn document_query_result_type_alias() {
        // Verify that DocumentQueryResult is the canonical result type
        fn check_alias() -> DocumentQueryResult<String> {
            Ok("test".to_string())
        }
        // This just verifies the type alias compiles correctly
        let _ = check_alias();
    }

    #[test]
    fn sort_order_structure() {
        let sort = DocumentSortOrder {
            field: "created_at".to_string(),
            direction: SortDirection::Ascending,
        };

        assert_eq!(sort.field, "created_at");
        assert_eq!(sort.direction, SortDirection::Ascending);
    }

    #[test]
    fn sort_direction_equality() {
        assert_eq!(SortDirection::Ascending, SortDirection::Ascending);
        assert_eq!(SortDirection::Descending, SortDirection::Descending);
        assert_ne!(SortDirection::Ascending, SortDirection::Descending);
    }
}
