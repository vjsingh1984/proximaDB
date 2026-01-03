//! # Query Facade Adapter
//!
//! Protocol-agnostic adapter for UnifiedQueryFacade.
//! Provides conversion between protocol-specific request/response types
//! and the unified QueryRequest/QueryResult types.
//!
//! ## Purpose
//!
//! This adapter serves as a thin wrapper that:
//! 1. Converts REST/gRPC proto requests to `QueryRequest`
//! 2. Executes through `UnifiedQueryFacade`
//! 3. Converts `QueryResult` back to protocol-specific responses
//!
//! ## Architecture
//!
//! ```text
//! REST/gRPC Handler
//!        ↓
//! QueryFacadeAdapter.vector_search(VectorSearchRequest)
//!        ↓
//! QueryRequest::vector_search(...)
//!        ↓
//! UnifiedQueryFacade.execute(QueryRequest)
//!        ↓
//! QueryResult
//!        ↓
//! VectorOperationResponse
//! ```

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use tracing::{debug, instrument};

use serde::{Deserialize, Serialize};

use crate::proto::proximadb_v1::{
    VectorSearchRequest, VectorOperationResponse, SearchResult, SearchVectorRecord,
};
use super::{
    UnifiedQueryFacade, QueryRequest, QueryResult, QueryResultData,
};

/// Result of explaining a query's execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainResult {
    /// Components involved in the query
    pub components: Vec<ExplainComponent>,
    /// Fusion strategy to be used
    pub fusion_strategy: String,
    /// Estimated total cost (max of component costs for parallel execution)
    pub estimated_total_cost: f64,
    /// Name of the strategy that will handle this query
    pub strategy_name: String,
    /// Whether this is a multi-model query
    pub is_multi_model: bool,
}

/// Execution plan component for a single data model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainComponent {
    /// Data model (Vector, Graph, Document, Observability, Relational)
    pub model: String,
    /// Estimated execution cost
    pub estimated_cost: f64,
    /// Whether this component can be parallelized with others
    pub parallelizable: bool,
}

/// Adapter for routing protocol-specific requests through UnifiedQueryFacade
///
/// This adapter provides protocol-agnostic methods that:
/// - Accept proto request types (VectorSearchRequest, etc.)
/// - Convert to unified QueryRequest
/// - Execute through the facade
/// - Convert results back to proto response types
#[derive(Clone)]
pub struct QueryFacadeAdapter {
    facade: Arc<UnifiedQueryFacade>,
}

impl QueryFacadeAdapter {
    /// Create a new adapter wrapping the given facade
    pub fn new(facade: Arc<UnifiedQueryFacade>) -> Self {
        Self { facade }
    }

    /// Get a reference to the underlying facade
    pub fn facade(&self) -> &Arc<UnifiedQueryFacade> {
        &self.facade
    }

    /// Execute vector search through the unified facade
    ///
    /// Converts VectorSearchRequest to QueryRequest, executes, and converts back.
    #[instrument(skip(self, request), fields(collection = %request.collection_id, top_k = request.top_k))]
    pub async fn vector_search(
        &self,
        request: VectorSearchRequest,
    ) -> Result<VectorOperationResponse> {
        let start = Instant::now();

        // Extract query vector from request
        let query_vector = request.queries
            .first()
            .map(|q| q.vector.clone())
            .unwrap_or_default();

        if query_vector.is_empty() {
            return Err(anyhow!("No query vector provided"));
        }

        let top_k = request.top_k as usize;
        let collection_id = request.collection_id.clone();

        debug!(
            vector_dims = query_vector.len(),
            top_k = top_k,
            collection = %collection_id,
            "Converting VectorSearchRequest to QueryRequest"
        );

        // Create QueryRequest from proto request
        let query_request = QueryRequest::vector_search(query_vector, top_k)
            .with_target(&collection_id);

        // Execute through facade
        let result = self.facade.execute(query_request).await?;

        // Convert QueryResult to VectorOperationResponse
        let response = self.query_result_to_vector_response(result)?;

        debug!(
            results = response.results.as_ref().map(|r| r.results.len()).unwrap_or(0),
            elapsed_ms = start.elapsed().as_millis(),
            "Vector search completed via adapter"
        );

        Ok(response)
    }

    /// Execute SQL query through the unified facade
    ///
    /// Returns the QueryResult directly (protocol handlers can format as needed)
    #[instrument(skip(self), fields(sql_len = sql.len()))]
    pub async fn sql_query(&self, sql: &str) -> Result<QueryResult> {
        debug!("Executing SQL query via adapter");
        let query_request = QueryRequest::sql(sql);
        self.facade.execute(query_request).await
    }

    /// Execute federated query (SQL with multi-model extensions)
    ///
    /// Supports VECTOR_SEARCH, GRAPH_QUERY, DOCUMENT_QUERY, LOGS, METRICS extensions
    #[instrument(skip(self), fields(sql_len = sql.len()))]
    pub async fn federated_query(&self, sql: &str) -> Result<QueryResult> {
        debug!("Executing federated query via adapter");
        let query_request = QueryRequest::federated(sql);
        self.facade.execute(query_request).await
    }

    /// Execute graph query through the unified facade
    ///
    /// Supports Cypher-like query syntax
    #[instrument(skip(self), fields(query_len = cypher.len()))]
    pub async fn graph_query(
        &self,
        cypher: &str,
        graph_name: Option<&str>,
    ) -> Result<QueryResult> {
        debug!(
            graph_name = ?graph_name,
            "Executing graph query via adapter"
        );

        let mut query_request = QueryRequest::graph(cypher);
        if let Some(name) = graph_name {
            query_request = query_request.with_target(name);
        }

        self.facade.execute(query_request).await
    }

    /// Explain a query's execution plan without executing it
    ///
    /// Analyzes the query and returns the planned execution strategy,
    /// estimated costs, and component breakdown.
    #[instrument(skip(self), fields(sql_len = sql.len()))]
    pub fn explain(&self, sql: &str) -> Result<ExplainResult> {
        debug!("Explaining query via adapter");

        // Create a federated query request to analyze
        let query_request = QueryRequest::federated(sql);

        // Find which strategy would handle this query
        let strategy_name = self.facade.strategy_names()
            .into_iter()
            .find(|name| {
                // Check if this strategy can handle the query type
                *name == "federated" || *name == "sql" || *name == "vector"
            })
            .unwrap_or("unknown")
            .to_string();

        // Parse the query to detect multi-model extensions
        let sql_upper = sql.to_uppercase();
        let mut components = Vec::new();
        let mut estimated_cost: f64 = 1.0;

        // Detect VECTOR_SEARCH
        if sql_upper.contains("VECTOR_SEARCH") || sql.contains("<->") || sql.contains("::vector") {
            components.push(ExplainComponent {
                model: "Vector".to_string(),
                estimated_cost: 1.0,
                parallelizable: true,
            });
            estimated_cost = estimated_cost.max(1.0_f64);
        }

        // Detect GRAPH_QUERY
        if sql_upper.contains("GRAPH_QUERY") {
            components.push(ExplainComponent {
                model: "Graph".to_string(),
                estimated_cost: 3.0,
                parallelizable: true,
            });
            estimated_cost = estimated_cost.max(3.0_f64);
        }

        // Detect DOCUMENT_QUERY
        if sql_upper.contains("DOCUMENT_QUERY") {
            components.push(ExplainComponent {
                model: "Document".to_string(),
                estimated_cost: 2.0,
                parallelizable: true,
            });
            estimated_cost = estimated_cost.max(2.0_f64);
        }

        // Detect LOGS/METRICS
        if sql_upper.contains("LOGS(") || sql_upper.contains("METRICS(") {
            components.push(ExplainComponent {
                model: "Observability".to_string(),
                estimated_cost: 2.5,
                parallelizable: true,
            });
            estimated_cost = estimated_cost.max(2.5_f64);
        }

        // If no multi-model extensions detected, it's a standard SQL query
        if components.is_empty() {
            components.push(ExplainComponent {
                model: "Relational".to_string(),
                estimated_cost: 1.0,
                parallelizable: false,
            });
        }

        // Detect fusion strategy from query (if UNION is present)
        let fusion_strategy = if sql_upper.contains("UNION ALL") {
            "Union".to_string()
        } else if sql_upper.contains("INTERSECT") {
            "Intersection".to_string()
        } else if components.len() > 1 {
            "Intersection".to_string() // Default for multi-model
        } else {
            "None".to_string()
        };

        Ok(ExplainResult {
            components,
            fusion_strategy,
            estimated_total_cost: estimated_cost,
            strategy_name,
            is_multi_model: sql_upper.contains("VECTOR_SEARCH")
                || sql_upper.contains("GRAPH_QUERY")
                || sql_upper.contains("DOCUMENT_QUERY")
                || sql_upper.contains("LOGS(")
                || sql_upper.contains("METRICS(")
                || sql.contains("<->"),
        })
    }

    /// Convert QueryResult to VectorOperationResponse proto
    fn query_result_to_vector_response(
        &self,
        result: QueryResult,
    ) -> Result<VectorOperationResponse> {
        let mut search_records = Vec::new();

        match result.data {
            QueryResultData::VectorResults(matches) => {
                for m in matches {
                    search_records.push(SearchVectorRecord {
                        id: m.id,
                        score: m.score as f64,
                        vector: vec![], // Don't return vectors by default to save bandwidth
                        metadata: std::collections::HashMap::new(),
                        version: None,
                        similarity: Some(m.score),
                        timestamp: None,
                        source: None,
                        expanded_context: vec![],
                        semantic_similarity: None,
                        quantization_info: None,
                        engine_stats: std::collections::HashMap::new(),
                        index_path: None,
                    });
                }
            }
            QueryResultData::Rows(rows) => {
                // Convert JSON rows to search records if possible
                for row in rows {
                    if let Some(obj) = row.as_object() {
                        let id = obj.get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let score = obj.get("score")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);

                        search_records.push(SearchVectorRecord {
                            id,
                            score,
                            vector: vec![],
                            metadata: std::collections::HashMap::new(),
                            version: None,
                            similarity: Some(score as f32),
                            timestamp: None,
                            source: None,
                            expanded_context: vec![],
                            semantic_similarity: None,
                            quantization_info: None,
                            engine_stats: std::collections::HashMap::new(),
                            index_path: None,
                        });
                    }
                }
            }
            _ => {
                // For other result types, return empty results
                debug!("Query returned non-vector results, returning empty vectors");
            }
        }

        let total_found = search_records.len() as i64;

        Ok(VectorOperationResponse {
            success: true,
            operation: 1, // Search operation
            metrics: None,
            results: Some(SearchResult {
                results: search_records,
                total_found,
                collection_id: None,
            }),
            vector_ids: vec![],
            error_message: None,
            error_code: None,
        })
    }
}

// ================================================================================
// TESTS
// ================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::facade::{FacadeConfig, QueryStrategy, QueryContext, VectorMatch};
    use crate::proto::proximadb_v1::SearchQuery;
    use async_trait::async_trait;

    /// Mock strategy for testing
    struct MockVectorStrategy;

    #[async_trait]
    impl QueryStrategy for MockVectorStrategy {
        fn name(&self) -> &str { "mock_vector" }
        fn can_handle(&self, request: &QueryRequest) -> bool {
            matches!(request.query_type, crate::query::facade::QueryType::VectorSearch)
        }
        fn priority(&self) -> i32 { 100 }

        async fn execute(&self, _request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
            Ok(QueryResult {
                data: QueryResultData::VectorResults(vec![
                    VectorMatch {
                        id: "vec1".to_string(),
                        score: 0.95,
                        metadata: None,
                    },
                    VectorMatch {
                        id: "vec2".to_string(),
                        score: 0.87,
                        metadata: None,
                    },
                ]),
                metrics: None,
            })
        }
    }

    fn create_test_adapter() -> QueryFacadeAdapter {
        let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
            Arc::new(MockVectorStrategy),
        ];
        let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
        QueryFacadeAdapter::new(facade)
    }

    #[tokio::test]
    async fn test_vector_search_converts_request() {
        let adapter = create_test_adapter();

        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            top_k: 10,
            queries: vec![SearchQuery {
                vector: vec![0.1, 0.2, 0.3],
                filters: std::collections::HashMap::new(),
                advanced_filter: None,
            }],
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        let response = adapter.vector_search(request).await.unwrap();

        assert!(response.success);
        assert!(response.results.is_some());
        let results = response.results.unwrap();
        assert_eq!(results.results.len(), 2);
        assert_eq!(results.results[0].id, "vec1");
        assert!((results.results[0].score - 0.95).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_vector_search_empty_vector_error() {
        let adapter = create_test_adapter();

        let request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            top_k: 10,
            queries: vec![], // No query vector
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        let result = adapter.vector_search(request).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No query vector"));
    }

    #[test]
    fn test_adapter_creation() {
        let adapter = create_test_adapter();
        assert!(Arc::strong_count(adapter.facade()) >= 1);
    }

    #[test]
    fn test_adapter_clone() {
        let adapter = create_test_adapter();
        let cloned = adapter.clone();
        assert!(Arc::ptr_eq(adapter.facade(), cloned.facade()));
    }
}
