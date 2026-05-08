//! Compatibility shim for the extracted graph subset crate.
//!
//! The supported graph-subset parsing and read-only execution helpers now live
//! in the `proximadb-graph-subset` workspace crate. This module preserves the
//! historical root import path while the remaining root callers are migrated to
//! depend on the leaf crate directly.

#![allow(unused_imports)]

pub(crate) use proximadb_graph_subset::{
    ExecutedGraphQuery, GraphExecutionStats, ParsedGraphQuery, SupportedGraphQueryDescriptor,
    describe_supported_graph_query, discover_default_graph_id, execute_supported_graph_query,
    execute_supported_graph_query_with_start_nodes, parse_supported_graph_query,
};
