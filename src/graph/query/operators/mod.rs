//! Compatibility shim for graph query operators.
//!
//! The actual operator implementation now lives in the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface
//! while keeping the operator/runtime layer on a smaller, graph-scoped build
//! path.

use crate::graph::engines::GraphEngine;
use anyhow::Result;
use proximadb_graph::query::storage::GraphQueryStorage;
use proximadb_proto::proximadb_v1::{Edge, Node};
use std::sync::Arc;

pub use proximadb_graph::query::operators::{
    ComparisonOperator, EdgeDirection, ExpandOperator, FilterExpression, FilterOperator,
    FilterValue, LimitOperator, NodeScanOperator, ProjectOperator, ProjectionSpec,
    evaluate_property_filter,
};

pub mod expand {
    pub use proximadb_graph::query::operators::expand::*;
}

pub mod filter {
    pub use proximadb_graph::query::operators::filter::*;
}

pub mod limit {
    pub use proximadb_graph::query::operators::limit::*;
}

pub mod project {
    pub use proximadb_graph::query::operators::project::*;
}

pub mod scan {
    pub use proximadb_graph::query::operators::scan::*;
}

pub use proximadb_graph::query::storage::GraphQueryStorage as QueryStorage;

/// Adapter from the root `GraphEngine` contract to the extracted query-storage
/// contract used by `proximadb-graph`.
pub struct GraphEngineQueryStorageAdapter<E: GraphEngine + ?Sized> {
    engine: Arc<E>,
}

impl<E: GraphEngine + ?Sized> GraphEngineQueryStorageAdapter<E> {
    /// Create a new adapter from a graph engine.
    pub fn new(engine: Arc<E>) -> Self {
        Self { engine }
    }
}

impl<E: GraphEngine + ?Sized> GraphQueryStorage for GraphEngineQueryStorageAdapter<E> {
    fn get_node(&self, id: &str) -> Result<Option<Arc<Node>>> {
        self.engine.get_node(&id.to_string()).map_err(Into::into)
    }

    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
        self.engine.get_nodes_by_label(label).map_err(Into::into)
    }

    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
        self.engine.get_all_nodes().map_err(Into::into)
    }

    fn get_outgoing_edges(&self, node_id: &str, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>> {
        self.engine
            .get_outgoing_edges(&node_id.to_string(), edge_type)
            .map_err(Into::into)
    }

    fn get_incoming_edges(&self, node_id: &str, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>> {
        self.engine
            .get_incoming_edges(&node_id.to_string(), edge_type)
            .map_err(Into::into)
    }
}

/// Convert a root graph engine into the extracted query-storage contract.
pub fn graph_query_storage<E: GraphEngine + ?Sized + 'static>(
    engine: Arc<E>,
) -> Arc<dyn GraphQueryStorage> {
    Arc::new(GraphEngineQueryStorageAdapter::new(engine))
}
