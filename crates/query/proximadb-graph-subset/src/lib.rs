//! Compatibility facade for the extracted graph subset query runtime.
//!
//! The implementation now lives in `proximadb-query` so graph subset parsing
//! and execution can be reused by the cross-model query runtime without an
//! upward dependency edge.

pub use proximadb_query::graph_lowering::{
    LoweredGraphQuery, ParsedGraphQuery, SupportedGraphQueryDescriptor,
    describe_supported_graph_query, lower_supported_graph_query,
    lower_supported_graph_query_component, lower_supported_graph_query_expr,
    parse_supported_graph_query,
};
pub use proximadb_query::graph_runtime::{
    ExecutedGraphQuery, GraphExecutionStats, GraphQueryRuntimeResult, LoweredGraphQueryResult,
    discover_default_graph_id, execute_graph_query_expr, execute_graph_query_expr_with_start_nodes,
    execute_lowered_graph_query, execute_lowered_graph_query_with_start_nodes,
    execute_supported_graph_query, execute_supported_graph_query_with_start_nodes,
    graph_query_row_id, legacy_graph_row_to_node, shape_graph_query_row,
};
