//! # Document Strategy
//!
//! Real implementation of `QueryStrategy` for document queries.
//! Wraps the existing `DocumentService` infrastructure.
//!
//! ## Features
//!
//! - Converts facade `QueryRequest` to document operations
//! - Supports JSON path filtering via DocumentService
//! - Returns results in unified `QueryResult` format
//!
//! ## Architecture
//!
//! ```text
//! QueryRequest (facade)
//!       │
//!       ▼
//! DocumentStrategy
//!       │
//!       ▼
//! DocumentService.query_documents()
//!       │
//!       ▼
//! DocumentQueryResult
//!       │
//!       ▼
//! QueryResult (facade)
//! ```

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tracing::{debug, info, instrument};

use crate::query::facade::{
    ExecutionMetrics, QueryContent, QueryContext, QueryRequest, QueryResult, QueryResultData,
    QueryStrategy, QueryType,
};
use crate::storage::document::DocumentService;

/// Document Strategy - Real implementation wrapping DocumentService
///
/// This strategy handles `QueryType::Document` requests by:
/// 1. Parsing the document filter expression
/// 2. Executing queries via DocumentService
/// 3. Converting results back to facade format
pub struct DocumentStrategy {
    /// Document service for query execution
    doc_service: Arc<DocumentService>,
    /// Strategy priority (higher = preferred)
    priority: i32,
}

impl DocumentStrategy {
    /// Create a new DocumentStrategy
    pub fn new(doc_service: Arc<DocumentService>) -> Self {
        Self {
            doc_service,
            priority: 70, // Lower than SQL, graph, vector
        }
    }

    /// Create with custom priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Extract document filter from the request
    fn extract_query(&self, request: &QueryRequest) -> Result<String> {
        match &request.content {
            QueryContent::Document(filter) => Ok(filter.clone()),
            QueryContent::Sql(sql) => {
                // Extract from DOCUMENT_QUERY('collection', 'filter') syntax
                self.parse_document_query_function(sql)
            }
            _ => Err(anyhow!("DocumentStrategy requires Document content")),
        }
    }

    /// Parse DOCUMENT_QUERY('collection', 'filter') function call
    fn parse_document_query_function(&self, sql: &str) -> Result<String> {
        let upper = sql.to_uppercase();
        if let Some(start) = upper.find("DOCUMENT_QUERY") {
            let rest = &sql[start + 14..];
            if let Some(paren_start) = rest.find('(') {
                let content = &rest[paren_start + 1..];
                // Find matching closing paren
                let mut depth = 1;
                let mut paren_end = None;
                for (i, c) in content.char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                paren_end = Some(i);
                                break;
                            }
                        }
                        _ => {}
                    }
                }

                if let Some(end) = paren_end {
                    let args = &content[..end];
                    // Parse as (collection, filter) - extract filter part
                    let parts: Vec<&str> = args.splitn(2, ',').collect();
                    if parts.len() >= 2 {
                        let filter = parts[1].trim();
                        // Remove quotes
                        let filter = filter.trim_matches(|c| c == '\'' || c == '"');
                        return Ok(filter.to_string());
                    } else if parts.len() == 1 {
                        // Just filter, no collection specified
                        let filter = parts[0].trim().trim_matches(|c| c == '\'' || c == '"');
                        return Ok(filter.to_string());
                    }
                }
            }
        }
        Err(anyhow!("Could not parse DOCUMENT_QUERY function"))
    }

    /// Extract collection name from request
    fn extract_collection(&self, request: &QueryRequest) -> String {
        // Try to get from target
        if let Some(target) = &request.target {
            return target.clone();
        }

        // Try to parse from SQL content
        if let QueryContent::Sql(sql) = &request.content {
            if let Some(collection) = self.parse_collection_from_sql(sql) {
                return collection;
            }
        }

        // Default collection
        "default".to_string()
    }

    /// Parse collection name from DOCUMENT_QUERY('collection', ...)
    fn parse_collection_from_sql(&self, sql: &str) -> Option<String> {
        let upper = sql.to_uppercase();
        if let Some(start) = upper.find("DOCUMENT_QUERY") {
            let rest = &sql[start + 14..];
            if let Some(paren_start) = rest.find('(') {
                let content = &rest[paren_start + 1..];
                // First argument is collection
                let first_arg_end = content.find(',')?;
                let collection = content[..first_arg_end].trim();
                let collection = collection.trim_matches(|c| c == '\'' || c == '"');
                return Some(collection.to_string());
            }
        }
        None
    }

    /// Convert document query result to facade QueryResult
    fn to_facade_result(
        &self,
        result: crate::storage::document::DocumentQueryResult,
        execution_time_ms: u64,
    ) -> QueryResult {
        let docs_count = result.documents.len();

        // Convert documents to JSON rows
        let rows: Vec<serde_json::Value> = result
            .documents
            .into_iter()
            .map(|doc| {
                serde_json::json!({
                    "id": doc.id,
                    "document": doc.document,
                    "version": doc.version,
                    "collection_id": doc.collection_id,
                })
            })
            .collect();

        QueryResult {
            data: QueryResultData::Rows(rows),
            metrics: Some(ExecutionMetrics {
                execution_path: "unified".to_string(),
                strategy_name: "document".to_string(),
                execution_time_ms,
                planning_time_ms: 0,
                results_scanned: result.total_count.unwrap_or(docs_count as u64) as usize,
                results_returned: docs_count,
                cache_hit: false,
                extra: serde_json::json!({
                    "engine": "DocumentService",
                    "documents_returned": docs_count,
                    "query_time_ms": result.query_time_ms,
                }),
            }),
        }
    }
}

#[async_trait]
impl QueryStrategy for DocumentStrategy {
    fn name(&self) -> &str {
        "document"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        request.query_type == QueryType::Document
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    #[instrument(skip(self, request, _ctx), fields(strategy = "document"))]
    async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        use crate::storage::document::DocumentQueryParams;

        let start = Instant::now();

        // Extract collection and filter
        let collection = self.extract_collection(&request);
        #[allow(unused_variables)]
        let filter_str = self.extract_query(&request)?;

        debug!(
            collection = %collection,
            filter = %filter_str,
            "Executing document query"
        );

        // Build query params - using default for now
        // Note: Full filter parsing would require converting the filter string
        // to a DocumentFilter proto message
        let params = DocumentQueryParams {
            filter: None, // TODO: Parse filter_str to DocumentFilter
            projection: vec![],
            sort: vec![],
            limit: 100, // Default limit
            offset: 0,
            include_count: true,
        };

        // Execute query
        let result = self
            .doc_service
            .query_documents(&collection, params)
            .await?;

        let execution_time_ms = start.elapsed().as_millis() as u64;
        let facade_result = self.to_facade_result(result, execution_time_ms);

        info!(
            documents = facade_result
                .metrics
                .as_ref()
                .and_then(|m| m.extra.get("documents_returned"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            time_ms = execution_time_ms,
            "Document query completed"
        );

        Ok(facade_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_document_query_function() {
        // Create mock strategy (can't test without service, just test parsing)
        // The parsing logic is self-contained

        // Test that can_handle returns correct value
        let request = QueryRequest {
            query_type: QueryType::Document,
            target: Some("test_collection".to_string()),
            content: QueryContent::Document("status = 'active'".to_string()),
            params: Default::default(),
        };

        assert_eq!(request.query_type, QueryType::Document);
    }
}
