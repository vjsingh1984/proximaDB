pub mod ast;
pub mod cypher_ast;
pub mod cypher_functions;
pub mod cypher_parser;
pub mod execution;
pub mod executor;
pub mod operators;
pub mod parser;
pub mod pattern;
pub mod planner;
pub mod service;
pub mod storage;
pub mod traversal;
pub mod unified_parser;

use proximadb_kernel::error::ProximaDBError;

pub use proximadb_graph_query::{
    GraphQueryContext as QueryContext, GraphQueryExecutionResult as QueryExecutionResult,
    GraphQueryRow as QueryRow, GraphQueryStats as QueryStats,
};

/// Canonical result type for extracted graph-query modules.
pub type QueryResult<T> = std::result::Result<T, ProximaDBError>;
