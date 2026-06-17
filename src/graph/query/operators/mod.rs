//! Compatibility shim for graph query operators.
//!
//! The actual operator implementation now lives in the `proximadb-graph`
//! workspace crate. This module preserves the historical root import surface
//! while keeping the operator/runtime layer on a smaller, graph-scoped build
//! path.

use crate::graph::engines::GraphEngine;
use anyhow::Result;
use proximadb_proto::proximadb_v1::{Edge, Node};
use std::sync::Arc;

// TODO: Move implementation to proximadb-graph crate
// Stub implementations for compatibility

/// Comparison operator for filters
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonOperator {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Edge direction for traversal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDirection {
    Outgoing,
    Incoming,
    Both,
}

/// Expand operator for graph traversal
#[derive(Debug, Clone)]
pub struct ExpandOperator {
    pub from_var: String,
    pub edge_type: Option<String>,
    pub to_var: String,
    pub direction: EdgeDirection,
}

/// Filter expression
#[derive(Debug, Clone)]
pub enum FilterExpression {
    Comparison {
        variable: String,
        property: String,
        op: ComparisonOperator,
        value: FilterValue,
    },
    And(Box<FilterExpression>, Box<FilterExpression>),
    Or(Box<FilterExpression>, Box<FilterExpression>),
    Not(Box<FilterExpression>),
}

/// Filter operator
#[derive(Debug, Clone)]
pub struct FilterOperator {
    pub expression: FilterExpression,
}

/// Filter value
#[derive(Debug, Clone)]
pub enum FilterValue {
    Null,
    Bool(bool),
    Int64(i64),
    Float64(f64),
    String(String),
}

/// Limit operator
#[derive(Debug, Clone)]
pub struct LimitOperator {
    pub limit: usize,
}

/// Node scan operator
#[derive(Debug, Clone)]
pub struct NodeScanOperator {
    pub labels: Option<Vec<String>>,
    pub filters: Vec<FilterExpression>,
}

/// Projection specification
#[derive(Debug, Clone)]
pub struct ProjectionSpec {
    pub variable: String,
    pub property: Option<String>,
    pub alias: Option<String>,
}

/// Project operator
#[derive(Debug, Clone)]
pub struct ProjectOperator {
    pub projections: Vec<ProjectionSpec>,
}

/// Evaluate property filter (stub)
pub fn evaluate_property_filter(_node: &Node, _expression: &FilterExpression) -> Result<bool> {
    Ok(true)
}

// Sub-modules with stub exports
pub mod expand {
    pub use super::ExpandOperator;
}

pub mod filter {
    pub use super::{
        ComparisonOperator, FilterExpression, FilterOperator, FilterValue, evaluate_property_filter,
    };
}

pub mod limit {
    pub use super::LimitOperator;
}

pub mod project {
    pub use super::{ProjectOperator, ProjectionSpec};
}

pub mod scan {
    pub use super::NodeScanOperator;
}

/// Graph query storage trait
pub trait GraphQueryStorage: Send + Sync {
    fn get_node(&self, id: &str) -> Result<Option<Arc<Node>>>;
    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>>;
    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>>;
    fn get_outgoing_edges(&self, node_id: &str, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>>;
    fn get_incoming_edges(&self, node_id: &str, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>>;
}

pub type QueryStorage = dyn GraphQueryStorage;

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
