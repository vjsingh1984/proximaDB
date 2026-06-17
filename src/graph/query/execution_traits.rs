use crate::graph::engines::GraphEngine;
use std::sync::Arc;

// TODO: Move implementation to proximadb-graph crate
// Stub implementations for compatibility

/// Column specification for query results
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnSpec {
    pub name: String,
    pub value_type: ValueType,
}

/// Per-physical-operator execution statistics for graph query traits.
///
/// Naming note: this type used to be called `ExecutionStats` and collided
/// with the federated/router/proto `ExecutionStats` types. Renamed because
/// the field set is operator-scoped (rows + time only). Distinct from
/// `proximadb_query::graph_runtime::GraphExecutionStats`, which captures
/// query-level counts (rows + matched nodes + matched edges). The proto
/// `proximadb.explain.v1::ExecutionStats` remains the canonical EXPLAIN
/// form per ADR-004.
#[derive(Debug, Clone, Default)]
pub struct GraphOperatorExecutionStats {
    pub rows_processed: usize,
    pub execution_time_ms: u64,
}

/// Path element for graph traversal results
#[derive(Debug, Clone)]
pub struct PathElement {
    pub node_id: String,
    pub edge_id: Option<String>,
}

/// Physical query operator
#[derive(Debug, Clone)]
pub enum PhysicalOperator {
    Scan,
    Filter,
    Project,
    Join,
    Aggregate,
    Sort,
    Limit,
}

/// Query value
#[derive(Debug, Clone)]
pub enum QueryValue {
    Null,
    Bool(bool),
    Int64(i64),
    Float64(f64),
    String(String),
    Node(String),
    Edge(String),
    Path(Vec<PathElement>),
}

/// Result tuple
pub type ResultTuple = Vec<(String, QueryValue)>;

/// Value type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Null,
    Bool,
    Int64,
    Float64,
    String,
    Node,
    Edge,
    Path,
}

/// Query execution context
///
/// Provides context for query execution including:
/// - Graph engine for data access
/// - Timeout and resource limits
/// - Profiling and tracing settings
pub struct ExecutionContext<E: GraphEngine + ?Sized = dyn GraphEngine> {
    /// Graph engine for data access
    pub engine: Arc<E>,

    /// Maximum execution time (milliseconds)
    pub timeout_ms: Option<u64>,

    /// Maximum rows to return
    pub limit: Option<usize>,

    /// Enable query profiling
    pub profile: bool,

    /// Execution statistics (accumulated during execution)
    pub stats: GraphOperatorExecutionStats,
}

impl<E: GraphEngine + ?Sized> ExecutionContext<E> {
    /// Create new execution context
    pub fn new(engine: Arc<E>) -> Self {
        Self {
            engine,
            timeout_ms: None,
            limit: None,
            profile: false,
            stats: GraphOperatorExecutionStats::default(),
        }
    }

    /// Set timeout in milliseconds
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Set result limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Enable profiling
    pub fn with_profiling(mut self) -> Self {
        self.profile = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Edge, Node};
    use proximadb_kernel::error::ProximaDBError;

    struct MockEngine;

    #[async_trait::async_trait]
    impl GraphEngine for MockEngine {
        async fn insert_node(&self, node: Node) -> Result<Arc<Node>, ProximaDBError> {
            Ok(Arc::new(node))
        }

        fn get_node(&self, _id: &String) -> Result<Option<Arc<Node>>, ProximaDBError> {
            Ok(None)
        }

        async fn update_node(&self, node: Node) -> Result<Arc<Node>, ProximaDBError> {
            Ok(Arc::new(node))
        }

        async fn delete_node(&self, _id: &String) -> Result<Option<Arc<Node>>, ProximaDBError> {
            Ok(None)
        }

        async fn insert_edge(&self, edge: Edge) -> Result<Arc<Edge>, ProximaDBError> {
            Ok(Arc::new(edge))
        }

        fn get_edge(&self, _id: &String) -> Result<Option<Arc<Edge>>, ProximaDBError> {
            Ok(None)
        }

        async fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>, ProximaDBError> {
            Ok(Arc::new(edge))
        }

        async fn delete_edge(&self, _id: &String) -> Result<Option<Arc<Edge>>, ProximaDBError> {
            Ok(None)
        }

        fn get_outgoing_edges(
            &self,
            _node_id: &String,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>, ProximaDBError> {
            Ok(vec![])
        }

        fn get_incoming_edges(
            &self,
            _node_id: &String,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>, ProximaDBError> {
            Ok(vec![])
        }

        fn get_neighbors(
            &self,
            _node_id: &String,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Ok(vec![])
        }

        fn get_nodes_by_label(&self, _label: &str) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Ok(vec![])
        }

        fn node_count(&self) -> Result<usize, ProximaDBError> {
            Ok(0)
        }

        fn edge_count(&self) -> Result<usize, ProximaDBError> {
            Ok(0)
        }

        fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Ok(vec![])
        }
    }

    #[test]
    fn test_execution_context_builder_chain() {
        let engine: Arc<dyn GraphEngine> = Arc::new(MockEngine);
        let context = ExecutionContext::new(engine.clone())
            .with_timeout(250)
            .with_limit(10)
            .with_profiling();

        assert!(Arc::ptr_eq(&context.engine, &engine));
        assert_eq!(context.timeout_ms, Some(250));
        assert_eq!(context.limit, Some(10));
        assert!(context.profile);
        assert_eq!(context.stats.rows_processed, 0);
        assert_eq!(context.stats.execution_time_ms, 0);
    }
}
