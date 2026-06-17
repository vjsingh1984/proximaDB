use proximadb_query_filter::{FilterOperator, FilterValue};

/// Graph traversal expression used by cross-model query orchestration.
#[derive(Debug, Clone)]
pub struct GraphTraversalExpr {
    /// Graph name.
    pub graph_name: String,
    /// Start node(s).
    pub start_nodes: StartNodeSpec,
    /// Edge type(s) to traverse.
    pub edge_types: Vec<String>,
    /// Traversal direction.
    pub direction: TraversalDirection,
    /// Maximum depth.
    pub max_depth: u32,
    /// Minimum depth.
    pub min_depth: u32,
    /// Node filters.
    pub node_filters: Vec<NodeFilter>,
    /// Edge filters.
    pub edge_filters: Vec<EdgeFilter>,
    /// Return paths or just nodes.
    pub return_paths: bool,
}

/// Start node specification.
#[derive(Debug, Clone)]
pub enum StartNodeSpec {
    /// Specific node IDs.
    Ids(Vec<String>),
    /// Nodes matching a label.
    Label(String),
    /// Nodes matching a filter.
    Filter(NodeFilter),
    /// Start nodes sourced from another query component.
    FromComponent(usize),
}

/// Traversal direction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TraversalDirection {
    #[default]
    Outgoing,
    Incoming,
    Both,
}

/// Node filter for graph traversals.
#[derive(Debug, Clone)]
pub struct NodeFilter {
    /// Label to match.
    pub label: Option<String>,
    /// Property filters.
    pub properties: Vec<PropertyFilter>,
}

/// Edge filter for graph traversals.
#[derive(Debug, Clone)]
pub struct EdgeFilter {
    /// Edge type to match.
    pub edge_type: Option<String>,
    /// Property filters.
    pub properties: Vec<PropertyFilter>,
    /// Weight range (min, max).
    pub weight_range: Option<(f64, f64)>,
}

/// Property filter for graph traversals.
#[derive(Debug, Clone)]
pub struct PropertyFilter {
    /// Property name.
    pub name: String,
    /// Comparison operator.
    pub operator: FilterOperator,
    /// Comparison value.
    pub value: FilterValue,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_direction_defaults_to_outgoing() {
        assert_eq!(TraversalDirection::default(), TraversalDirection::Outgoing);
    }

    #[test]
    fn start_node_filter_carries_nested_property_filters() {
        let spec = StartNodeSpec::Filter(NodeFilter {
            label: Some("Person".to_string()),
            properties: vec![PropertyFilter {
                name: "name".to_string(),
                operator: FilterOperator::Eq,
                value: FilterValue::String("Alice".to_string()),
            }],
        });

        match spec {
            StartNodeSpec::Filter(filter) => {
                assert_eq!(filter.label.as_deref(), Some("Person"));
                assert_eq!(filter.properties.len(), 1);
            }
            other => panic!("expected filter start node spec, got {:?}", other),
        }
    }
}
