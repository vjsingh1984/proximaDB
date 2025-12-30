//! # Federated Query Engine
//!
//! Provides unified query execution across all data models (Vector, Document, Graph, RDBMS, Observability).
//! This enables true cross-model queries where vector similarity search results can be joined with
//! graph traversals, document lookups, and observability data in a single SQL query.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    FEDERATED QUERY ENGINE                        │
//! │  PostgreSQL Wire Protocol + SQL Extensions + Federated Execution │
//! └─────────────────────────────────────────────────────────────────┘
//!                               │
//!         ┌─────────────────────┼─────────────────────┐
//!         ▼                     ▼                     ▼
//! ┌───────────────┐    ┌───────────────┐    ┌───────────────┐
//! │ FederatedParser│    │CrossModelOpt  │    │FederatedExec  │
//! │ SQL+Extensions │    │ Cost Model    │    │ Arrow Batches │
//! └───────────────┘    └───────────────┘    └───────────────┘
//! ```
//!
//! ## SQL Extensions
//!
//! ProximaDB extends standard SQL with special functions for each data model:
//!
//! ### VECTOR_SEARCH(collection, query_vector, top_k)
//!
//! Performs vector similarity search using the configured distance metric.
//!
//! ```sql
//! -- Find 10 most similar products to a query vector
//! SELECT * FROM VECTOR_SEARCH('products', '[0.1, 0.2, 0.3, ...]', 10);
//! ```
//!
//! ### GRAPH_QUERY('cypher_query')
//!
//! Executes a Cypher-like graph traversal query.
//!
//! ```sql
//! -- Find all friends of friends
//! SELECT * FROM GRAPH_QUERY('MATCH (a:Person)-[:KNOWS]->(b)-[:KNOWS]->(c) RETURN c.name');
//!
//! -- Get connections with specific relationship types
//! SELECT * FROM GRAPH_QUERY('MATCH (p:Person {name: "Alice"})-[:WORKS_AT]->(c:Company) RETURN c');
//! ```
//!
//! ### DOCUMENT_QUERY(collection, filter)
//!
//! Queries document collections with optional filter expressions.
//!
//! ```sql
//! -- Find documents matching a filter
//! SELECT * FROM DOCUMENT_QUERY('orders', 'status = "pending"');
//! ```
//!
//! ### LOGS(namespace) / METRICS(namespace)
//!
//! Queries observability data (logs and metrics).
//!
//! ```sql
//! -- Query recent logs
//! SELECT * FROM LOGS('production') WHERE timestamp > now() - interval '1h';
//!
//! -- Query metrics
//! SELECT * FROM METRICS('system') WHERE metric_name = 'cpu_usage';
//! ```
//!
//! ### Vector Distance Operator (<->)
//!
//! pgvector-compatible distance operator for ORDER BY clauses.
//!
//! ```sql
//! -- Sort by vector distance (pgvector syntax)
//! SELECT * FROM products ORDER BY embedding <-> '[0.1,0.2,...]'::vector LIMIT 10;
//! ```
//!
//! ## Cross-Model Queries
//!
//! The real power comes from combining multiple data models in a single query:
//!
//! ```sql
//! -- Find similar products for a user, then get related reviews
//! SELECT u.name, v.product_id, v.score, d.review_text
//! FROM users u
//! JOIN LATERAL VECTOR_SEARCH('products', u.preference_vector, 10) v ON true
//! JOIN LATERAL DOCUMENT_QUERY('reviews', concat('product_id = "', v.product_id, '"')) d ON true;
//!
//! -- Correlate graph relationships with vector similarity
//! SELECT g.person_name, v.similar_items
//! FROM GRAPH_QUERY('MATCH (p:Person)-[:PURCHASED]->(item) RETURN p.name as person_name, item.id') g
//! JOIN LATERAL VECTOR_SEARCH('items', g.item_embedding, 5) v ON true;
//!
//! -- Monitor system health with observability + graph context
//! SELECT l.timestamp, l.message, g.service_dependencies
//! FROM LOGS('errors') l
//! JOIN LATERAL GRAPH_QUERY('MATCH (s:Service {name: "' || l.service || '"})-[:DEPENDS_ON]->(d) RETURN d.name') g ON true
//! WHERE l.timestamp > now() - interval '5m';
//! ```
//!
//! ## API Access
//!
//! Federated queries are available via:
//!
//! - **REST**: `POST /api/v1/unified/federated` with `{ "query": "SELECT ..." }`
//! - **gRPC**: `SqlService.ExecuteSql` with federated SQL
//! - **PostgreSQL Wire Protocol**: Connect with psql and run queries directly
//!
//! ## Example Usage (REST)
//!
//! ```bash
//! curl -X POST http://localhost:5678/api/v1/unified/federated \
//!   -H "Content-Type: application/json" \
//!   -d '{"query": "SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1, 0.2]', 10)"}'
//! ```

pub mod parser;
pub mod optimizer;
pub mod execution;

// Re-exports
pub use parser::{FederatedParser, FederatedQuery, QueryType};
pub use optimizer::{CrossModelOptimizer, QueryPlan, PlanNode};
pub use execution::{FederatedExecutor, ExecutionResult};

use std::sync::Arc;
use anyhow::Result;

use crate::storage::multimodel::MultiModelStorageFacade;

/// Federated query context containing all necessary components
pub struct FederatedQueryContext {
    /// Multi-model storage facade
    pub storage: Arc<MultiModelStorageFacade>,
    /// Parser for SQL with extensions
    pub parser: FederatedParser,
    /// Cross-model optimizer
    pub optimizer: CrossModelOptimizer,
    /// Federated executor
    pub executor: FederatedExecutor,
}

impl FederatedQueryContext {
    /// Create a new federated query context
    pub fn new(storage: Arc<MultiModelStorageFacade>) -> Self {
        Self {
            storage: storage.clone(),
            parser: FederatedParser::new(),
            optimizer: CrossModelOptimizer::new(),
            executor: FederatedExecutor::new(storage),
        }
    }

    /// Execute a federated query
    pub async fn execute(&self, sql: &str) -> Result<ExecutionResult> {
        // 1. Parse the query
        let federated_query = self.parser.parse(sql)?;

        // 2. Optimize the query plan
        let plan = self.optimizer.optimize(&federated_query)?;

        // 3. Execute the plan
        self.executor.execute(plan).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_federated_context_creation() {
        let storage = Arc::new(MultiModelStorageFacade::new());
        let ctx = FederatedQueryContext::new(storage);
        assert!(ctx.parser.supported_extensions().len() > 0);
    }
}
