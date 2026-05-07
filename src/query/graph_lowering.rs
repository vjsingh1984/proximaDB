//! Shared lowering helpers for declarative graph queries.
//!
//! This module bridges the supported graph query subset parser to the
//! multi-model query IR so facade, federated, and unified surfaces can
//! reuse the same graph target resolution and output-shape contract.

use anyhow::Result;

use crate::query::graph_subset::describe_supported_graph_query;
use crate::query::unified::ast::{DataModel, GraphQueryExpr, ModelOperation, QueryComponent};

/// Lower a supported declarative graph query into the unified query IR.
pub(crate) fn lower_supported_graph_query_expr(
    query: &str,
    request_target: Option<&str>,
    default_graph: Option<&str>,
) -> Result<GraphQueryExpr> {
    let descriptor = describe_supported_graph_query(query, request_target, default_graph)?;

    Ok(GraphQueryExpr {
        graph_name: descriptor.graph_id().to_string(),
        normalized_query: descriptor.normalized_query().to_string(),
        output_columns: descriptor.output_columns().to_vec(),
        uses_legacy_node_rows: descriptor.uses_legacy_node_rows(),
        max_depth: descriptor.max_depth(),
    })
}

/// Lower a supported declarative graph query into a graph query component.
pub(crate) fn lower_supported_graph_query_component(
    query: &str,
    request_target: Option<&str>,
    default_graph: Option<&str>,
) -> Result<QueryComponent> {
    Ok(QueryComponent {
        model: DataModel::Graph,
        operation: ModelOperation::GraphQuery(lower_supported_graph_query_expr(
            query,
            request_target,
            default_graph,
        )?),
        filters: Vec::new(),
        dependencies: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_supported_graph_query_with_from_clause() {
        let component = lower_supported_graph_query_component(
            "MATCH (n:Person) FROM social RETURN n.name AS person_name",
            None,
            Some("default"),
        )
        .expect("graph query should lower");

        match component.operation {
            ModelOperation::GraphQuery(expr) => {
                assert_eq!(expr.graph_name, "social");
                assert_eq!(
                    expr.normalized_query,
                    "MATCH (n:Person) RETURN n.name AS person_name"
                );
                assert_eq!(expr.output_columns, vec!["person_name".to_string()]);
                assert!(!expr.uses_legacy_node_rows);
                assert_eq!(expr.max_depth, 0);
            }
            other => panic!("expected graph query operation, got {:?}", other),
        }
    }

    #[test]
    fn lowering_rejects_conflicting_request_target() {
        let error = lower_supported_graph_query_component(
            "MATCH (n) FROM social RETURN n",
            Some("api_graph"),
            Some("default"),
        )
        .expect_err("conflicting graph targets should fail");

        assert!(error.to_string().contains("target conflict"));
    }
}
