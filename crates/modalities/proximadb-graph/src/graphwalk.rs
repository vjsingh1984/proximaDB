//! Agent-facing graph traversal contracts.
//!
//! GraphWalk is a tool interface over canonical graph records and rebuildable
//! topology projections. It keeps agent navigation out of prompt-sized subgraph
//! dumps while leaving durable authority with `ProximaRecord` edge/node records.

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use proximadb_records::ProximaRecord;
use serde::{Deserialize, Serialize};

use crate::{EdgeDirection, NodeId};

/// Bounded traversal request issued by an agent or query planner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphWalkRequest {
    /// Graph or collection name in xCatalog.
    pub graph_name: String,
    /// Starting node ids.
    pub start_nodes: Vec<NodeId>,
    /// Maximum traversal depth.
    pub max_depth: u32,
    /// Direction to expand at each step.
    pub direction: EdgeDirection,
    /// Optional edge type allow-list.
    pub edge_types: Vec<String>,
    /// Maximum returned steps after scoring/filtering.
    pub limit: u32,
    /// Tenant/RLS id propagated from the query context.
    pub tenant_id: Option<String>,
    /// Planner-supplied hints such as projection freshness or scoring mode.
    pub hints: HashMap<String, String>,
}

impl GraphWalkRequest {
    /// Construct a bounded graph walk from one or more starting nodes.
    pub fn new(graph_name: impl Into<String>, start_nodes: Vec<NodeId>) -> Self {
        Self {
            graph_name: graph_name.into(),
            start_nodes,
            max_depth: 2,
            direction: EdgeDirection::Outgoing,
            edge_types: Vec::new(),
            limit: 32,
            tenant_id: None,
            hints: HashMap::new(),
        }
    }

    /// Attach tenant context for engine-level RLS/predicate pushdown.
    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Validate request bounds before a planner or agent tool executes it.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.graph_name.is_empty() {
            return Err("graph_name must not be empty");
        }
        if self.start_nodes.is_empty() {
            return Err("start_nodes must not be empty");
        }
        if self.max_depth == 0 {
            return Err("max_depth must be > 0");
        }
        if self.limit == 0 {
            return Err("limit must be > 0");
        }
        if let Some(tenant_id) = &self.tenant_id
            && tenant_id.is_empty()
        {
            return Err("tenant_id must not be empty when provided");
        }
        if self.edge_types.iter().any(|edge_type| edge_type.is_empty()) {
            return Err("edge_types must not contain empty values");
        }
        Ok(())
    }
}

/// One explained traversal step returned to an agent/tool caller.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphWalkStep {
    /// Source node id.
    pub source: NodeId,
    /// Target node id.
    pub target: NodeId,
    /// Edge type followed.
    pub edge_type: String,
    /// Depth from the request start node.
    pub depth: u32,
    /// Optional score assigned by semantic/topological ranking.
    pub score: Option<f32>,
    /// Canonical source/edge/target records available for downstream fusion.
    pub records: Vec<ProximaRecord>,
}

/// Result of a bounded graph walk.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GraphWalkResult {
    /// Traversal steps in planner-selected order.
    pub steps: Vec<GraphWalkStep>,
    /// Whether the result came from a fresh topology projection.
    pub projection_fresh: bool,
    /// Projection epoch used, when applicable.
    pub projection_epoch: Option<u64>,
}

impl GraphWalkResult {
    /// Validate freshness metadata returned by a graph walk implementation.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.projection_fresh && self.projection_epoch.is_none() {
            return Err("fresh graphwalk results must include projection_epoch");
        }
        for step in &self.steps {
            if step.edge_type.is_empty() {
                return Err("graphwalk step edge_type must not be empty");
            }
            if step.depth == 0 {
                return Err("graphwalk step depth must be > 0");
            }
        }
        Ok(())
    }
}

/// Tool contract implemented by graph services or projection-backed executors.
#[async_trait]
pub trait GraphWalkTool: Send + Sync {
    /// Execute a bounded graph walk and return canonical records plus
    /// projection freshness metadata.
    async fn walk(&self, request: GraphWalkRequest) -> Result<GraphWalkResult>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphwalk_request_carries_tenant_and_bounds() {
        let start = NodeId::new();
        let request = GraphWalkRequest::new("agents", vec![start]).with_tenant("tenant-a");

        assert_eq!(request.graph_name, "agents");
        assert_eq!(request.start_nodes, vec![start]);
        assert_eq!(request.max_depth, 2);
        assert_eq!(request.limit, 32);
        assert_eq!(request.tenant_id.as_deref(), Some("tenant-a"));
        assert!(request.validate().is_ok());
    }

    #[test]
    fn graphwalk_request_rejects_unbounded_inputs() {
        let mut request = GraphWalkRequest::new("", Vec::new());
        request.max_depth = 0;
        request.limit = 0;

        assert_eq!(request.validate(), Err("graph_name must not be empty"));
    }

    #[test]
    fn graphwalk_result_requires_epoch_for_fresh_projection() {
        let result = GraphWalkResult {
            steps: Vec::new(),
            projection_fresh: true,
            projection_epoch: None,
        };

        assert_eq!(
            result.validate(),
            Err("fresh graphwalk results must include projection_epoch")
        );
    }
}
