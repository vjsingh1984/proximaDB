//! Shared lowering helpers for declarative graph queries.
//!
//! This module now acts as a thin compatibility adapter from the extracted
//! `proximadb-graph-subset` lowering contract into the root unified query IR.

use anyhow::Result;
use proximadb_graph_subset::lower_supported_graph_query;

use crate::query::unified::ast::{DataModel, GraphQueryExpr, ModelOperation, QueryComponent};

/// Lower a supported declarative graph query into the unified query IR.
pub(crate) fn lower_supported_graph_query_expr(
    query: &str,
    request_target: Option<&str>,
    default_graph: Option<&str>,
) -> Result<GraphQueryExpr> {
    lower_supported_graph_query(query, request_target, default_graph)
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

    #[test]
    fn lowering_uses_default_graph_and_legacy_shape_for_node_projection() {
        let expr = lower_supported_graph_query_expr(
            "MATCH (n:Person)-[:KNOWS]->(m:Person) RETURN m",
            None,
            Some("social"),
        )
        .expect("graph query should lower");

        assert_eq!(expr.graph_name, "social");
        assert_eq!(
            expr.normalized_query,
            "MATCH (n:Person)-[:KNOWS]->(m:Person) RETURN m"
        );
        assert_eq!(
            expr.output_columns,
            vec![
                "node_id".to_string(),
                "label".to_string(),
                "properties".to_string()
            ]
        );
        assert!(expr.uses_legacy_node_rows);
        assert_eq!(expr.max_depth, 1);
    }
}
