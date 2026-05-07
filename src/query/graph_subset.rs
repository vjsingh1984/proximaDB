//! Shared support for the read-only graph query subset used by the facade and
//! federated SQL extensions.

// Module provides shared graph query infrastructure for facade and federated SQL.
// Functions will be wired in when full graph SQL integration is complete.
#![allow(dead_code)]

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde_json::{Map, Value};

use crate::graph::query::ast::{
    CompiledPattern, EdgeDirection, PropertyConstraint, PropertyProjection, WhereClause,
};
use crate::graph::query::parser::QueryParser;
use crate::graph::service::GraphOperationsService;
use crate::graph::{Edge, Node};
use crate::proto::proximadb_v1::{
    EdgeQuery, NodeQuery, PropertyFilter, PropertyFilterOperator, PropertyValue, property_value,
};

#[derive(Debug, Clone)]
pub(crate) struct ParsedGraphQuery {
    graph_id: String,
    normalized_query: String,
    compiled: CompiledPattern,
    output_columns: Vec<String>,
}

impl ParsedGraphQuery {
    pub(crate) fn graph_id(&self) -> &str {
        &self.graph_id
    }

    pub(crate) fn normalized_query(&self) -> &str {
        &self.normalized_query
    }

    pub(crate) fn output_columns(&self) -> &[String] {
        &self.output_columns
    }

    pub(crate) fn uses_legacy_node_rows(&self) -> bool {
        if self.compiled.return_spec.projections.len() != 1 {
            return false;
        }

        match &self.compiled.return_spec.projections[0] {
            PropertyProjection::Variable(variable) => self
                .compiled
                .nodes
                .iter()
                .any(|node| node.variable == *variable),
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SupportedGraphQueryDescriptor {
    graph_id: String,
    normalized_query: String,
    output_columns: Vec<String>,
    uses_legacy_node_rows: bool,
    max_depth: u32,
}

impl SupportedGraphQueryDescriptor {
    pub(crate) fn graph_id(&self) -> &str {
        &self.graph_id
    }

    pub(crate) fn normalized_query(&self) -> &str {
        &self.normalized_query
    }

    pub(crate) fn output_columns(&self) -> &[String] {
        &self.output_columns
    }

    pub(crate) fn uses_legacy_node_rows(&self) -> bool {
        self.uses_legacy_node_rows
    }

    pub(crate) fn max_depth(&self) -> u32 {
        self.max_depth
    }
}

pub(crate) fn describe_supported_graph_query(
    query: &str,
    request_target: Option<&str>,
    default_graph: Option<&str>,
) -> Result<SupportedGraphQueryDescriptor> {
    let parsed = parse_supported_graph_query(query, request_target, default_graph)?;
    let uses_legacy_node_rows = parsed.uses_legacy_node_rows();
    let output_columns = if uses_legacy_node_rows {
        vec![
            "node_id".to_string(),
            "label".to_string(),
            "properties".to_string(),
        ]
    } else {
        parsed.output_columns().to_vec()
    };

    Ok(SupportedGraphQueryDescriptor {
        graph_id: parsed.graph_id().to_string(),
        normalized_query: parsed.normalized_query().to_string(),
        output_columns,
        uses_legacy_node_rows,
        max_depth: parsed.compiled.edges.len() as u32,
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct GraphExecutionStats {
    pub(crate) rows_returned: usize,
    pub(crate) matched_nodes: usize,
    pub(crate) matched_edges: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ExecutedGraphQuery {
    pub(crate) rows: Vec<Value>,
    pub(crate) stats: GraphExecutionStats,
}

#[derive(Debug, Clone)]
enum BoundValue {
    Node(Arc<Node>),
    Edge(Arc<Edge>),
}

impl BoundValue {
    fn as_node(&self) -> Option<&Arc<Node>> {
        match self {
            Self::Node(node) => Some(node),
            Self::Edge(_) => None,
        }
    }

    fn identity(&self) -> (&'static str, &str) {
        match self {
            Self::Node(node) => ("node", &node.id),
            Self::Edge(edge) => ("edge", &edge.id),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::Node(node) => node_to_json(node),
            Self::Edge(edge) => edge_to_json(edge),
        }
    }
}

type BindingRow = HashMap<String, BoundValue>;

pub(crate) async fn discover_default_graph_id(
    graph_ops: &GraphOperationsService,
) -> Option<String> {
    let graphs = graph_ops.list_graphs().await.ok()?;
    if graphs.is_empty() {
        None
    } else if graphs.iter().any(|graph| graph == "default") {
        Some("default".to_string())
    } else {
        graphs.into_iter().next()
    }
}

pub(crate) fn parse_supported_graph_query(
    query: &str,
    request_target: Option<&str>,
    default_graph: Option<&str>,
) -> Result<ParsedGraphQuery> {
    let (normalized_query, from_graph) = strip_from_clause(query)?;

    let request_target = request_target.filter(|target| !target.trim().is_empty());
    if let (Some(target), Some(from_graph)) = (request_target, from_graph.as_deref())
        && target != from_graph
    {
        return Err(anyhow!(
            "Graph query target conflict: request targets graph '{}' but query specifies FROM {}",
            target,
            from_graph
        ));
    }

    let graph_id = request_target
        .or(from_graph.as_deref())
        .or(default_graph)
        .unwrap_or("default")
        .to_string();

    let parser = QueryParser::new();
    let compiled = parser.parse(&normalized_query).map_err(|error| {
        anyhow!(
            "Unsupported graph query syntax for the facade/federated subset: {}",
            error
        )
    })?;

    validate_compiled_query(&compiled, &normalized_query)?;

    Ok(ParsedGraphQuery {
        graph_id,
        normalized_query,
        output_columns: compiled.return_spec.variables.clone(),
        compiled,
    })
}

pub(crate) async fn execute_supported_graph_query(
    graph_ops: &GraphOperationsService,
    parsed: &ParsedGraphQuery,
) -> Result<ExecutedGraphQuery> {
    execute_supported_graph_query_with_start_nodes(graph_ops, parsed, None).await
}

pub(crate) async fn execute_supported_graph_query_with_start_nodes(
    graph_ops: &GraphOperationsService,
    parsed: &ParsedGraphQuery,
    start_node_ids: Option<&[String]>,
) -> Result<ExecutedGraphQuery> {
    let compiled = &parsed.compiled;
    let start_pattern = &compiled.nodes[0];
    let mut bindings =
        initial_bindings(graph_ops, parsed.graph_id(), start_pattern, start_node_ids).await?;

    for (edge_pattern, next_node_pattern) in
        compiled.edges.iter().zip(compiled.nodes.iter().skip(1))
    {
        let mut next_bindings = Vec::new();

        for binding in bindings {
            let Some(current_node) = binding
                .get(&edge_pattern.from_variable)
                .and_then(BoundValue::as_node)
                .cloned()
            else {
                return Err(anyhow!(
                    "Traversal variable '{}' is not bound to a node",
                    edge_pattern.from_variable
                ));
            };

            let candidate_edges =
                query_candidate_edges(graph_ops, parsed.graph_id(), &current_node.id, edge_pattern)
                    .await?;

            for edge in candidate_edges {
                let next_node_id = resolve_adjacent_node_id(
                    &current_node.id,
                    edge.as_ref(),
                    edge_pattern.direction,
                );
                let Some(next_node) = graph_ops.get_node(parsed.graph_id(), &next_node_id).await?
                else {
                    continue;
                };

                if !matches_node_pattern(next_node.as_ref(), next_node_pattern) {
                    continue;
                }

                let mut next_binding = binding.clone();
                let edge_value = BoundValue::Edge(edge.clone());
                if let Some(edge_var) = &edge_pattern.variable {
                    if !binding_is_compatible(next_binding.get(edge_var), &edge_value) {
                        continue;
                    }
                    next_binding.insert(edge_var.clone(), edge_value);
                } else {
                    // No explicit edge variable — still track the edge under a
                    // synthetic key so matched_edges stats count it correctly
                    let synthetic_key = format!("_edge_{}", edge.id);
                    next_binding.insert(synthetic_key, edge_value);
                }

                let next_node_value = BoundValue::Node(next_node.clone());
                if !binding_is_compatible(
                    next_binding.get(&next_node_pattern.variable),
                    &next_node_value,
                ) {
                    continue;
                }
                next_binding.insert(next_node_pattern.variable.clone(), next_node_value);
                next_bindings.push(next_binding);
            }
        }

        bindings = next_bindings;
        if bindings.is_empty() {
            break;
        }
    }

    bindings.retain(|binding| {
        compiled
            .where_clauses
            .iter()
            .all(|clause| matches_where_clause(binding, clause))
    });

    let matched_nodes = bindings
        .iter()
        .flat_map(|binding| binding.values())
        .filter_map(|value| match value {
            BoundValue::Node(node) => Some(node.id.clone()),
            BoundValue::Edge(_) => None,
        })
        .collect::<HashSet<_>>()
        .len();

    let matched_edges = bindings
        .iter()
        .flat_map(|binding| binding.values())
        .filter_map(|value| match value {
            BoundValue::Edge(edge) => Some(edge.id.clone()),
            BoundValue::Node(_) => None,
        })
        .collect::<HashSet<_>>()
        .len();

    let projected_rows = bindings
        .into_iter()
        .map(|binding| project_row(&binding, compiled))
        .collect::<Result<Vec<_>>>()?;
    let rows = apply_row_modifiers(projected_rows, compiled)?;

    Ok(ExecutedGraphQuery {
        stats: GraphExecutionStats {
            rows_returned: rows.len(),
            matched_nodes,
            matched_edges,
        },
        rows,
    })
}

async fn initial_bindings(
    graph_ops: &GraphOperationsService,
    graph_id: &str,
    start_pattern: &crate::graph::query::ast::NodePattern,
    start_node_ids: Option<&[String]>,
) -> Result<Vec<BindingRow>> {
    let start_nodes = if let Some(start_node_ids) = start_node_ids {
        query_candidate_start_nodes(graph_ops, graph_id, start_pattern, start_node_ids).await?
    } else {
        query_candidate_nodes(graph_ops, graph_id, start_pattern).await?
    };

    Ok(start_nodes
        .into_iter()
        .map(|node| {
            let mut row = BindingRow::new();
            row.insert(start_pattern.variable.clone(), BoundValue::Node(node));
            row
        })
        .collect())
}

fn strip_from_clause(query: &str) -> Result<(String, Option<String>)> {
    let trimmed = query.trim().trim_end_matches(';').trim();
    let upper = trimmed.to_uppercase();

    let Some(from_pos) = upper.find(" FROM ") else {
        return Ok((trimmed.to_string(), None));
    };

    let before_from = trimmed[..from_pos].trim_end();
    let after_from = trimmed[from_pos + 6..].trim_start();
    let graph_name_len = after_from
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .count();

    if graph_name_len == 0 {
        return Err(anyhow!("Expected graph name after FROM in graph query"));
    }

    let graph_name = after_from[..graph_name_len].to_string();
    let remainder = after_from[graph_name_len..].trim_start();
    let normalized_query = if remainder.is_empty() {
        before_from.to_string()
    } else {
        format!("{} {}", before_from, remainder)
    };

    Ok((normalized_query, Some(graph_name)))
}

fn validate_compiled_query(compiled: &CompiledPattern, normalized_query: &str) -> Result<()> {
    let upper = normalized_query.to_uppercase();
    for unsupported in [
        "OPTIONAL MATCH",
        "CREATE ",
        "DELETE ",
        "MERGE ",
        " SET ",
        " REMOVE ",
        " WITH ",
        " UNION ",
    ] {
        if upper.contains(unsupported) {
            return Err(anyhow!(
                "Unsupported graph query clause '{}' in facade/federated graph subset",
                unsupported.trim()
            ));
        }
    }

    if normalized_query.contains("[*") {
        return Err(anyhow!(
            "Variable-length graph paths are not supported in the facade/federated graph subset"
        ));
    }

    if compiled.nodes.is_empty() {
        return Err(anyhow!(
            "Graph query must contain at least one node pattern"
        ));
    }

    if !compiled.paths.is_empty() {
        return Err(anyhow!(
            "Explicit path bindings are not supported in the facade/federated graph subset"
        ));
    }

    if compiled.edges.is_empty() {
        if compiled.nodes.len() != 1 {
            return Err(anyhow!(
                "Disconnected graph matches are not supported; use a single node pattern or a linear traversal chain"
            ));
        }
    } else {
        if compiled.nodes.len() != compiled.edges.len() + 1 {
            return Err(anyhow!(
                "Only linear graph traversal chains are supported in the facade/federated graph subset"
            ));
        }

        for (index, edge) in compiled.edges.iter().enumerate() {
            if compiled.nodes[index].variable != edge.from_variable
                || compiled.nodes[index + 1].variable != edge.to_variable
            {
                return Err(anyhow!(
                    "Only linear graph traversal chains are supported in the facade/federated graph subset"
                ));
            }

            if edge
                .properties
                .values()
                .any(|constraint| !matches!(constraint, PropertyConstraint::Equals(_)))
            {
                return Err(anyhow!(
                    "Edge property maps only support equality constraints in the facade/federated graph subset"
                ));
            }
        }
    }

    let mut known_variables = HashSet::new();
    for node in &compiled.nodes {
        if node.optional {
            return Err(anyhow!(
                "OPTIONAL MATCH is not supported in the facade/federated graph subset"
            ));
        }
        if !known_variables.insert(node.variable.clone()) {
            return Err(anyhow!(
                "Repeated node variable '{}' is not supported in the facade/federated graph subset",
                node.variable
            ));
        }
        if node
            .properties
            .values()
            .any(|constraint| !matches!(constraint, PropertyConstraint::Equals(_)))
        {
            return Err(anyhow!(
                "Node property maps only support equality constraints in the facade/federated graph subset"
            ));
        }
    }

    for edge in &compiled.edges {
        if edge.optional {
            return Err(anyhow!(
                "OPTIONAL MATCH is not supported in the facade/federated graph subset"
            ));
        }
        if let Some(variable) = &edge.variable
            && !known_variables.insert(variable.clone())
        {
            return Err(anyhow!(
                "Repeated edge variable '{}' is not supported in the facade/federated graph subset",
                variable
            ));
        }
    }

    for clause in &compiled.where_clauses {
        validate_where_clause(clause, &known_variables)?;
    }

    for projection in &compiled.return_spec.projections {
        match projection {
            PropertyProjection::Variable(variable) => {
                if !known_variables.contains(variable) {
                    return Err(anyhow!(
                        "Graph query RETURN references unknown variable '{}'",
                        variable
                    ));
                }
            }
            PropertyProjection::Property { variable, .. } => {
                if !known_variables.contains(variable) {
                    return Err(anyhow!(
                        "Graph query RETURN references unknown variable '{}'",
                        variable
                    ));
                }
            }
            PropertyProjection::Count
            | PropertyProjection::Sum { .. }
            | PropertyProjection::Avg { .. }
            | PropertyProjection::Min { .. }
            | PropertyProjection::Max { .. } => {
                return Err(anyhow!(
                    "Aggregations are not supported in the facade/federated graph subset"
                ));
            }
        }
    }

    for (order_key, _) in &compiled.return_spec.order_by {
        if !compiled
            .return_spec
            .variables
            .iter()
            .any(|column| column == order_key)
        {
            return Err(anyhow!(
                "ORDER BY '{}' must reference a returned column alias in the facade/federated graph subset",
                order_key
            ));
        }
    }

    Ok(())
}

fn validate_where_clause(clause: &WhereClause, known_variables: &HashSet<String>) -> Result<()> {
    match clause {
        WhereClause::Property {
            variable,
            constraint,
            ..
        } => {
            if !known_variables.contains(variable) {
                return Err(anyhow!(
                    "Graph query WHERE references unknown variable '{}'",
                    variable
                ));
            }

            if !is_supported_constraint(constraint) {
                return Err(anyhow!(
                    "WHERE clause contains an unsupported constraint in the facade/federated graph subset"
                ));
            }
        }
        WhereClause::And(left, right) | WhereClause::Or(left, right) => {
            validate_where_clause(left, known_variables)?;
            validate_where_clause(right, known_variables)?;
        }
        WhereClause::Not(inner) => validate_where_clause(inner, known_variables)?,
    }

    Ok(())
}

fn is_supported_constraint(constraint: &PropertyConstraint) -> bool {
    matches!(
        constraint,
        PropertyConstraint::Equals(_)
            | PropertyConstraint::NotEquals(_)
            | PropertyConstraint::GreaterThan(_)
            | PropertyConstraint::GreaterThanOrEqual(_)
            | PropertyConstraint::GreaterOrEqual(_)
            | PropertyConstraint::LessThan(_)
            | PropertyConstraint::LessThanOrEqual(_)
            | PropertyConstraint::LessOrEqual(_)
            | PropertyConstraint::Contains(_)
            | PropertyConstraint::StartsWith(_)
            | PropertyConstraint::EndsWith(_)
            | PropertyConstraint::Exists
            | PropertyConstraint::NotExists
            | PropertyConstraint::In(_)
            | PropertyConstraint::NotIn(_)
            | PropertyConstraint::Regex(_)
    )
}

async fn query_candidate_nodes(
    graph_ops: &GraphOperationsService,
    graph_id: &str,
    pattern: &crate::graph::query::ast::NodePattern,
) -> Result<Vec<Arc<Node>>> {
    if let Some(id) = extract_identity_constraint(pattern.properties.get("id"))
        && let Some(node) = graph_ops.get_node(graph_id, &id).await?
    {
        if matches_node_pattern(node.as_ref(), pattern) {
            return Ok(vec![node]);
        }
        return Ok(Vec::new());
    }

    let filters = pattern
        .properties
        .iter()
        .filter_map(|(key, constraint)| property_filter_from_constraint(key, constraint))
        .collect::<Vec<_>>();

    let query = NodeQuery {
        graph_id: graph_id.to_string(),
        labels: pattern.labels.clone(),
        filters,
        limit: None,
        offset: None,
        continuation_token: None,
    };

    let mut nodes = graph_ops.query_nodes(graph_id, query).await?;
    nodes.retain(|node| matches_node_pattern(node.as_ref(), pattern));
    Ok(nodes)
}

async fn query_candidate_start_nodes(
    graph_ops: &GraphOperationsService,
    graph_id: &str,
    pattern: &crate::graph::query::ast::NodePattern,
    start_node_ids: &[String],
) -> Result<Vec<Arc<Node>>> {
    let mut nodes = Vec::new();
    let mut seen = HashSet::new();

    for node_id in start_node_ids {
        if !seen.insert(node_id.clone()) {
            continue;
        }

        let Some(node) = graph_ops.get_node(graph_id, node_id).await? else {
            continue;
        };

        if matches_node_pattern(node.as_ref(), pattern) {
            nodes.push(node);
        }
    }

    Ok(nodes)
}

async fn query_candidate_edges(
    graph_ops: &GraphOperationsService,
    graph_id: &str,
    current_node_id: &str,
    pattern: &crate::graph::query::ast::EdgePattern,
) -> Result<Vec<Arc<Edge>>> {
    let filters = pattern
        .properties
        .iter()
        .filter_map(|(key, constraint)| property_filter_from_constraint(key, constraint))
        .collect::<Vec<_>>();

    let mut collected = Vec::new();
    let mut seen = HashSet::new();

    let mut push_edges = |edges: Vec<Arc<Edge>>| {
        for edge in edges {
            if seen.insert(edge.id.clone()) {
                collected.push(edge);
            }
        }
    };

    match pattern.direction {
        EdgeDirection::Outgoing => {
            push_edges(
                graph_ops
                    .query_edges(
                        graph_id,
                        EdgeQuery {
                            graph_id: graph_id.to_string(),
                            from_node_id: Some(current_node_id.to_string()),
                            to_node_id: None,
                            edge_types: pattern.edge_types.clone(),
                            filters: filters.clone(),
                            limit: None,
                            offset: None,
                            continuation_token: None,
                        },
                    )
                    .await?,
            );
        }
        EdgeDirection::Incoming => {
            push_edges(
                graph_ops
                    .query_edges(
                        graph_id,
                        EdgeQuery {
                            graph_id: graph_id.to_string(),
                            from_node_id: None,
                            to_node_id: Some(current_node_id.to_string()),
                            edge_types: pattern.edge_types.clone(),
                            filters: filters.clone(),
                            limit: None,
                            offset: None,
                            continuation_token: None,
                        },
                    )
                    .await?,
            );
        }
        EdgeDirection::Bidirectional => {
            push_edges(
                graph_ops
                    .query_edges(
                        graph_id,
                        EdgeQuery {
                            graph_id: graph_id.to_string(),
                            from_node_id: Some(current_node_id.to_string()),
                            to_node_id: None,
                            edge_types: pattern.edge_types.clone(),
                            filters: filters.clone(),
                            limit: None,
                            offset: None,
                            continuation_token: None,
                        },
                    )
                    .await?,
            );
            push_edges(
                graph_ops
                    .query_edges(
                        graph_id,
                        EdgeQuery {
                            graph_id: graph_id.to_string(),
                            from_node_id: None,
                            to_node_id: Some(current_node_id.to_string()),
                            edge_types: pattern.edge_types.clone(),
                            filters,
                            limit: None,
                            offset: None,
                            continuation_token: None,
                        },
                    )
                    .await?,
            );
        }
    }

    collected.retain(|edge| matches_edge_pattern(current_node_id, edge.as_ref(), pattern));
    Ok(collected)
}

fn property_filter_from_constraint(
    key: &str,
    constraint: &PropertyConstraint,
) -> Option<PropertyFilter> {
    if matches!(key, "id" | "labels" | "type" | "source" | "target") {
        return None;
    }

    let PropertyConstraint::Equals(value) = constraint else {
        return None;
    };
    let value = json_to_property_value(value)?;

    Some(PropertyFilter {
        key: key.to_string(),
        operator: PropertyFilterOperator::Equals as i32,
        value: Some(value),
    })
}

fn extract_identity_constraint(constraint: Option<&PropertyConstraint>) -> Option<String> {
    let PropertyConstraint::Equals(value) = constraint? else {
        return None;
    };

    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn json_to_property_value(value: &Value) -> Option<PropertyValue> {
    let value = match value {
        Value::Null => return None,
        Value::Bool(value) => property_value::Value::BoolValue(*value),
        Value::Number(value) => {
            if let Some(number) = value.as_i64() {
                property_value::Value::IntValue(number)
            } else if let Some(number) = value.as_f64() {
                property_value::Value::DoubleValue(number)
            } else {
                return None;
            }
        }
        Value::String(value) => property_value::Value::StringValue(value.clone()),
        Value::Array(values) => {
            let values = values
                .iter()
                .filter_map(json_to_property_value)
                .collect::<Vec<_>>();
            property_value::Value::ArrayValue(crate::proto::proximadb_v1::PropertyArray { values })
        }
        Value::Object(values) => {
            let fields = values
                .iter()
                .filter_map(|(key, value)| {
                    json_to_property_value(value).map(|value| (key.clone(), value))
                })
                .collect::<HashMap<_, _>>();
            property_value::Value::ObjectValue(crate::proto::proximadb_v1::PropertyObject {
                fields,
            })
        }
    };

    Some(PropertyValue { value: Some(value) })
}

fn property_value_to_json(value: &PropertyValue) -> Value {
    match &value.value {
        Some(property_value::Value::StringValue(value)) => Value::String(value.clone()),
        Some(property_value::Value::IntValue(value)) => Value::Number((*value).into()),
        Some(property_value::Value::DoubleValue(value)) => {
            serde_json::Number::from_f64(*value).map_or(Value::Null, Value::Number)
        }
        Some(property_value::Value::BoolValue(value)) => Value::Bool(*value),
        Some(property_value::Value::BytesValue(bytes)) => {
            use base64::Engine;
            Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
        }
        Some(property_value::Value::ArrayValue(values)) => Value::Array(
            values
                .values
                .iter()
                .map(property_value_to_json)
                .collect::<Vec<_>>(),
        ),
        Some(property_value::Value::ObjectValue(object)) => Value::Object(
            object
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), property_value_to_json(value)))
                .collect::<Map<_, _>>(),
        ),
        Some(property_value::Value::VectorValue(vector)) => Value::Array(
            vector
                .values
                .iter()
                .map(|value| Value::from(*value))
                .collect::<Vec<_>>(),
        ),
        None => Value::Null,
    }
}

fn matches_node_pattern(node: &Node, pattern: &crate::graph::query::ast::NodePattern) -> bool {
    if !pattern
        .labels
        .iter()
        .all(|label| node.labels.iter().any(|candidate| candidate == label))
    {
        return false;
    }

    pattern.properties.iter().all(|(key, constraint)| {
        let actual = resolve_node_property(node, key);
        matches_constraint(&actual, constraint)
    })
}

fn matches_edge_pattern(
    current_node_id: &str,
    edge: &Edge,
    pattern: &crate::graph::query::ast::EdgePattern,
) -> bool {
    let direction_matches = match pattern.direction {
        EdgeDirection::Outgoing => edge.from_node_id == current_node_id,
        EdgeDirection::Incoming => edge.to_node_id == current_node_id,
        EdgeDirection::Bidirectional => {
            edge.from_node_id == current_node_id || edge.to_node_id == current_node_id
        }
    };

    direction_matches
        && (pattern.edge_types.is_empty()
            || pattern
                .edge_types
                .iter()
                .any(|edge_type| edge_type == &edge.edge_type))
        && pattern.properties.iter().all(|(key, constraint)| {
            let actual = resolve_edge_property(edge, key);
            matches_constraint(&actual, constraint)
        })
}

fn resolve_adjacent_node_id(
    current_node_id: &str,
    edge: &Edge,
    direction: EdgeDirection,
) -> String {
    match direction {
        EdgeDirection::Outgoing => edge.to_node_id.clone(),
        EdgeDirection::Incoming => edge.from_node_id.clone(),
        EdgeDirection::Bidirectional => {
            if edge.from_node_id == current_node_id {
                edge.to_node_id.clone()
            } else {
                edge.from_node_id.clone()
            }
        }
    }
}

fn binding_is_compatible(existing: Option<&BoundValue>, candidate: &BoundValue) -> bool {
    existing.is_none_or(|current| current.identity() == candidate.identity())
}

fn matches_where_clause(binding: &BindingRow, clause: &WhereClause) -> bool {
    match clause {
        WhereClause::Property {
            variable,
            property,
            constraint,
        } => binding.get(variable).is_some_and(|value| {
            matches_constraint(&resolve_entity_property(value, property), constraint)
        }),
        WhereClause::And(left, right) => {
            matches_where_clause(binding, left) && matches_where_clause(binding, right)
        }
        WhereClause::Or(left, right) => {
            matches_where_clause(binding, left) || matches_where_clause(binding, right)
        }
        WhereClause::Not(inner) => !matches_where_clause(binding, inner),
    }
}

fn matches_constraint(actual: &Value, constraint: &PropertyConstraint) -> bool {
    match constraint {
        PropertyConstraint::Equals(expected) => actual == expected,
        PropertyConstraint::NotEquals(expected) => actual != expected,
        PropertyConstraint::GreaterThan(expected) => {
            compare_json_values(actual, expected) == Some(Ordering::Greater)
        }
        PropertyConstraint::GreaterThanOrEqual(expected)
        | PropertyConstraint::GreaterOrEqual(expected) => matches!(
            compare_json_values(actual, expected),
            Some(Ordering::Greater | Ordering::Equal)
        ),
        PropertyConstraint::LessThan(expected) => {
            compare_json_values(actual, expected) == Some(Ordering::Less)
        }
        PropertyConstraint::LessThanOrEqual(expected)
        | PropertyConstraint::LessOrEqual(expected) => matches!(
            compare_json_values(actual, expected),
            Some(Ordering::Less | Ordering::Equal)
        ),
        PropertyConstraint::In(values) => values.contains(actual),
        PropertyConstraint::NotIn(values) => !values.contains(actual),
        PropertyConstraint::Contains(expected) => actual
            .as_str()
            .is_some_and(|value| value.contains(expected)),
        PropertyConstraint::StartsWith(expected) => actual
            .as_str()
            .is_some_and(|value| value.starts_with(expected)),
        PropertyConstraint::EndsWith(expected) => actual
            .as_str()
            .is_some_and(|value| value.ends_with(expected)),
        PropertyConstraint::Regex(pattern) => actual
            .as_str()
            .and_then(|value| {
                regex::Regex::new(pattern)
                    .ok()
                    .map(|regex| regex.is_match(value))
            })
            .unwrap_or(false),
        PropertyConstraint::Exists => !actual.is_null(),
        PropertyConstraint::NotExists => actual.is_null(),
    }
}

fn compare_json_values(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64()?.partial_cmp(&right.as_f64()?),
        (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
        (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        (Value::Null, _) => Some(Ordering::Less),
        (_, Value::Null) => Some(Ordering::Greater),
        _ => Some(left.to_string().cmp(&right.to_string())),
    }
}

fn resolve_entity_property(entity: &BoundValue, property: &str) -> Value {
    match entity {
        BoundValue::Node(node) => resolve_node_property(node, property),
        BoundValue::Edge(edge) => resolve_edge_property(edge, property),
    }
}

fn resolve_node_property(node: &Node, property: &str) -> Value {
    match property {
        "id" => Value::String(node.id.clone()),
        "labels" => Value::Array(
            node.labels
                .iter()
                .map(|label| Value::String(label.clone()))
                .collect::<Vec<_>>(),
        ),
        property => node
            .properties
            .get(property)
            .map_or(Value::Null, property_value_to_json),
    }
}

fn resolve_edge_property(edge: &Edge, property: &str) -> Value {
    match property {
        "id" => Value::String(edge.id.clone()),
        "source" | "from" | "from_node_id" => Value::String(edge.from_node_id.clone()),
        "target" | "to" | "to_node_id" => Value::String(edge.to_node_id.clone()),
        "type" | "edge_type" => Value::String(edge.edge_type.clone()),
        "weight" => edge
            .weight
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        property => edge
            .properties
            .get(property)
            .map_or(Value::Null, property_value_to_json),
    }
}

fn project_row(binding: &BindingRow, compiled: &CompiledPattern) -> Result<Value> {
    let mut row = Map::new();

    for (column, projection) in compiled
        .return_spec
        .variables
        .iter()
        .zip(compiled.return_spec.projections.iter())
    {
        let value = match projection {
            PropertyProjection::Variable(variable) => binding
                .get(variable)
                .map(BoundValue::to_json)
                .ok_or_else(|| {
                    anyhow!(
                        "Graph query projection references unknown variable '{}'",
                        variable
                    )
                })?,
            PropertyProjection::Property { variable, property } => binding
                .get(variable)
                .map(|value| resolve_entity_property(value, property))
                .ok_or_else(|| {
                    anyhow!(
                        "Graph query projection references unknown variable '{}'",
                        variable
                    )
                })?,
            PropertyProjection::Count
            | PropertyProjection::Sum { .. }
            | PropertyProjection::Avg { .. }
            | PropertyProjection::Min { .. }
            | PropertyProjection::Max { .. } => {
                return Err(anyhow!(
                    "Aggregations are not supported in the facade/federated graph subset"
                ));
            }
        };

        row.insert(column.clone(), value);
    }

    Ok(Value::Object(row))
}

fn apply_row_modifiers(mut rows: Vec<Value>, compiled: &CompiledPattern) -> Result<Vec<Value>> {
    if compiled.return_spec.distinct {
        let mut seen = HashSet::new();
        rows.retain(|row| {
            let key = serde_json::to_string(row).unwrap_or_default();
            seen.insert(key)
        });
    }

    if !compiled.return_spec.order_by.is_empty() {
        rows.sort_by(|left, right| {
            let left = left.as_object();
            let right = right.as_object();

            for (column, ascending) in &compiled.return_spec.order_by {
                let left_value = left.and_then(|row| row.get(column)).unwrap_or(&Value::Null);
                let right_value = right
                    .and_then(|row| row.get(column))
                    .unwrap_or(&Value::Null);
                let ordering =
                    compare_json_values(left_value, right_value).unwrap_or(Ordering::Equal);
                let ordering = if *ascending {
                    ordering
                } else {
                    ordering.reverse()
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }

            Ordering::Equal
        });
    }

    let skip = compiled.return_spec.skip.unwrap_or(0);
    let limit = compiled.return_spec.limit.unwrap_or(usize::MAX);

    Ok(rows.into_iter().skip(skip).take(limit).collect())
}

fn node_to_json(node: &Node) -> Value {
    Value::Object(Map::from_iter([
        ("id".to_string(), Value::String(node.id.clone())),
        (
            "labels".to_string(),
            Value::Array(
                node.labels
                    .iter()
                    .map(|label| Value::String(label.clone()))
                    .collect::<Vec<_>>(),
            ),
        ),
        (
            "properties".to_string(),
            Value::Object(
                node.properties
                    .iter()
                    .map(|(key, value)| (key.clone(), property_value_to_json(value)))
                    .collect::<Map<_, _>>(),
            ),
        ),
    ]))
}

fn edge_to_json(edge: &Edge) -> Value {
    Value::Object(Map::from_iter([
        ("id".to_string(), Value::String(edge.id.clone())),
        (
            "source".to_string(),
            Value::String(edge.from_node_id.clone()),
        ),
        ("target".to_string(), Value::String(edge.to_node_id.clone())),
        ("type".to_string(), Value::String(edge.edge_type.clone())),
        (
            "weight".to_string(),
            edge.weight
                .and_then(serde_json::Number::from_f64)
                .map_or(Value::Null, Value::Number),
        ),
        (
            "properties".to_string(),
            Value::Object(
                edge.properties
                    .iter()
                    .map(|(key, value)| (key.clone(), property_value_to_json(value)))
                    .collect::<Map<_, _>>(),
            ),
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::proto::proximadb_v1::{CreateGraphRequest, Node as ProtoNode, property_value};

    fn pv_string(value: &str) -> PropertyValue {
        PropertyValue {
            value: Some(property_value::Value::StringValue(value.to_string())),
        }
    }

    async fn seed_graph() -> Arc<GraphOperationsService> {
        let service = Arc::new(GraphOperationsService::new());
        service
            .create_graph_collection(CreateGraphRequest {
                graph_id: "social".to_string(),
                name: Some("social".to_string()),
                description: None,
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            })
            .await
            .expect("create graph");

        service
            .create_node(
                "social",
                ProtoNode {
                    id: "alice".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::from([("name".to_string(), pv_string("Alice"))]),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("create alice");

        service
            .create_node(
                "social",
                ProtoNode {
                    id: "bob".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::from([("name".to_string(), pv_string("Bob"))]),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("create bob");

        service
            .create_node(
                "social",
                ProtoNode {
                    id: "acme".to_string(),
                    labels: vec!["Company".to_string()],
                    properties: HashMap::from([("name".to_string(), pv_string("Acme"))]),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("create acme");

        service
            .create_edge(
                "social",
                Edge {
                    id: "edge_knows".to_string(),
                    from_node_id: "alice".to_string(),
                    to_node_id: "bob".to_string(),
                    edge_type: "KNOWS".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("create knows");

        service
            .create_edge(
                "social",
                Edge {
                    id: "edge_works_at".to_string(),
                    from_node_id: "bob".to_string(),
                    to_node_id: "acme".to_string(),
                    edge_type: "WORKS_AT".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            )
            .await
            .expect("create works_at");

        service
    }

    #[test]
    fn test_parse_supported_graph_query_extracts_from_clause() {
        let parsed = parse_supported_graph_query(
            "MATCH (n:Person) FROM social RETURN n.name AS name",
            None,
            None,
        )
        .expect("parse graph query");

        assert_eq!(parsed.graph_id(), "social");
        assert_eq!(
            parsed.normalized_query(),
            "MATCH (n:Person) RETURN n.name AS name"
        );
        assert_eq!(parsed.output_columns(), &["name".to_string()]);
    }

    #[tokio::test]
    async fn test_execute_supported_graph_query_multi_hop_projection() {
        let service = seed_graph().await;
        let parsed = parse_supported_graph_query(
            "MATCH (a:Person {id: \"alice\"})-[:KNOWS]->(b)-[:WORKS_AT]->(c:Company) RETURN b.name AS colleague, c.name AS company",
            None,
            Some("social"),
        )
        .expect("parse graph query");

        let executed = execute_supported_graph_query(service.as_ref(), &parsed)
            .await
            .expect("execute graph query");

        assert_eq!(executed.stats.rows_returned, 1);
        assert_eq!(executed.stats.matched_nodes, 3);
        assert_eq!(executed.stats.matched_edges, 2);
        assert_eq!(
            executed.rows,
            vec![serde_json::json!({
                "colleague": "Bob",
                "company": "Acme"
            })]
        );
    }
}

// ========== Graph Query Optimization Interface (TD-035 Phase 3) ==========

/// Graph-specific query optimizer for cost-based optimization
///
/// This module provides query optimization capabilities specific to graph queries,
/// integrating with ProximaDB's unified query optimizer while providing graph-specific
/// cost estimation and plan optimization.
///
/// # Architecture
///
/// ```text
/// Graph Query Plan
///        ↓
/// GraphQueryOptimizer
///    ↓           ↓
/// Cost Estimator   Statistics Provider
///    ↓           ↓
/// Optimized Plan with:
/// - Index selection (property indexes, label indexes)
/// - Traversal order optimization
/// - Predicate pushdown to graph engine
/// - Join reordering for multi-hop traversals
/// ```
pub struct GraphQueryOptimizer {
    /// Graph operations service for statistics
    graph_service: Arc<GraphOperationsService>,
    /// Cached graph statistics
    statistics_cache: HashMap<String, GraphStatistics>,
}

/// Statistics about a graph for cost estimation
#[derive(Debug, Clone)]
pub struct GraphStatistics {
    /// Total number of nodes
    pub node_count: usize,
    /// Total number of edges
    pub edge_count: usize,
    /// Average fanout (edges per node)
    pub avg_fanout: f64,
    /// Label distribution
    pub label_distribution: HashMap<String, f64>, // label → fraction of nodes
    /// Property index availability
    pub property_indexes: HashMap<String, PropertyIndexStats>,
    /// Edge type distribution
    pub edge_type_distribution: HashMap<String, f64>, // edge_type → fraction of edges
}

/// Statistics about a property index
#[derive(Debug, Clone)]
pub struct PropertyIndexStats {
    /// Property name
    pub property_name: String,
    /// Number of distinct values
    pub distinct_count: usize,
    /// Index selectivity (0.0 = highly selective, 1.0 = not selective)
    pub selectivity: f64,
    /// Whether index exists
    pub exists: bool,
}

/// Cost estimate for a graph operation
#[derive(Debug, Clone)]
pub struct GraphOperationCost {
    /// Estimated number of nodes to scan
    pub node_scan_cost: f64,
    /// Estimated number of edges to traverse
    pub edge_traversal_cost: f64,
    /// Estimated number of results
    pub result_count: f64,
    /// Total cost (weighted sum)
    pub total_cost: f64,
}

/// Optimization hints for graph queries
#[derive(Debug, Clone)]
pub struct GraphOptimizationHints {
    /// Suggest using property index
    pub use_property_index: Option<String>,
    /// Suggest traversal order
    pub traversal_order: Vec<String>,
    /// Suggest algorithm for traversal
    pub suggested_algorithm: TraversalAlgorithmHint,
    /// Predicate pushdown recommendations
    pub pushdown_predicates: Vec<String>,
}

/// Algorithm hint for traversal
#[derive(Debug, Clone)]
pub enum TraversalAlgorithmHint {
    /// Use BFS for unweighted shortest path
    BFS,
    /// Use DFS for deep traversal
    DFS,
    /// Use Dijkstra for weighted shortest path
    Dijkstra,
    /// Use A* for weighted shortest path
    AStar,
    /// Let executor decide based on query
    Auto,
}

impl GraphQueryOptimizer {
    /// Create a new graph query optimizer
    pub fn new(graph_service: Arc<GraphOperationsService>) -> Self {
        Self {
            graph_service,
            statistics_cache: HashMap::new(),
        }
    }

    /// Optimize a graph query plan
    ///
    /// # Arguments
    ///
    /// * `plan` - Original query plan
    /// * `graph_id` - Graph ID for statistics
    ///
    /// # Returns
    ///
    /// Optimization hints and estimated cost
    pub async fn optimize_query(
        &mut self,
        plan: &crate::graph::query::planner::QueryPlan,
        graph_id: &str,
    ) -> Result<(GraphOptimizationHints, GraphOperationCost), VectorDBError> {
        // Get or load graph statistics
        let stats = self.get_graph_statistics(graph_id).await?;

        // Analyze plan to estimate cost
        let cost = self.estimate_plan_cost(plan, &stats)?;

        // Generate optimization hints
        let hints = self.generate_hints(plan, &stats, &cost)?;

        Ok((hints, cost))
    }

    /// Get statistics for a graph, loading from cache or service
    async fn get_graph_statistics(
        &mut self,
        graph_id: &str,
    ) -> Result<GraphStatistics, VectorDBError> {
        // Check cache first
        if let Some(stats) = self.statistics_cache.get(graph_id) {
            return Ok(stats.clone());
        }

        // Load from graph service
        // TODO: Implement actual statistics collection from graph service
        // For now, return default statistics
        let stats = GraphStatistics {
            node_count: 10000,
            edge_count: 50000,
            avg_fanout: 5.0,
            label_distribution: HashMap::new(),
            property_indexes: HashMap::new(),
            edge_type_distribution: HashMap::new(),
        };

        // Cache for future use
        self.statistics_cache
            .insert(graph_id.to_string(), stats.clone());
        Ok(stats)
    }

    /// Estimate cost of executing a query plan
    fn estimate_plan_cost(
        &self,
        plan: &crate::graph::query::planner::QueryPlan,
        stats: &GraphStatistics,
    ) -> Result<GraphOperationCost, VectorDBError> {
        let mut node_scan_cost = 0.0;
        let mut edge_traversal_cost = 0.0;
        let mut result_count = stats.node_count as f64; // Start with all nodes

        for step in &plan.steps {
            match &step.step_type {
                crate::graph::query::planner::PlanStepType::NodeScan {
                    labels,
                    property_filters,
                } => {
                    // Estimate node scan cost based on selectivity
                    let selectivity =
                        self.estimate_node_selectivity(labels, property_filters, stats)?;
                    node_scan_cost += stats.node_count as f64 * selectivity;
                    result_count *= selectivity;
                }
                crate::graph::query::planner::PlanStepType::Traverse { max_depth, .. } => {
                    // Estimate edge traversal cost
                    let depth = max_depth.unwrap_or(1) as f64;
                    let traversal_cost = result_count * stats.avg_fanout * depth;
                    edge_traversal_cost += traversal_cost;
                    result_count *= stats.avg_fanout;
                }
                crate::graph::query::planner::PlanStepType::Filter { .. } => {
                    // Filters reduce result count
                    result_count *= 0.5; // Assume 50% selectivity
                }
                _ => {
                    // Other steps have minimal cost
                }
            }
        }

        // Total cost: weighted sum (scans are cheaper than traversals)
        let total_cost = node_scan_cost * 0.1 + edge_traversal_cost * 1.0;

        Ok(GraphOperationCost {
            node_scan_cost,
            edge_traversal_cost,
            result_count,
            total_cost,
        })
    }

    /// Estimate selectivity of node filters
    fn estimate_node_selectivity(
        &self,
        labels: &Option<Vec<String>>,
        property_filters: &[crate::graph::query::planner::PropertyFilter],
        stats: &GraphStatistics,
    ) -> Result<f64, VectorDBError> {
        let mut selectivity = 1.0;

        // Label selectivity
        if let Some(labels) = labels {
            if let Some(label) = labels.first() {
                if let Some(&label_frac) = stats.label_distribution.get(label) {
                    selectivity *= label_frac;
                } else {
                    // Unknown label - assume 10% selectivity
                    selectivity *= 0.1;
                }
            }
        }

        // Property filter selectivity
        for filter in property_filters {
            if let Some(index_stats) = stats.property_indexes.get(&filter.property_name) {
                if index_stats.exists {
                    selectivity *= index_stats.selectivity;
                } else {
                    // No index - assume 50% selectivity
                    selectivity *= 0.5;
                }
            } else {
                // Unknown property - assume 50% selectivity
                selectivity *= 0.5;
            }
        }

        Ok(selectivity.min(1.0))
    }

    /// Generate optimization hints based on plan and statistics
    fn generate_hints(
        &self,
        plan: &crate::graph::query::planner::QueryPlan,
        stats: &GraphStatistics,
        _cost: &GraphOperationCost,
    ) -> Result<GraphOptimizationHints, VectorDBError> {
        let mut hints = GraphOptimizationHints {
            use_property_index: None,
            traversal_order: Vec::new(),
            suggested_algorithm: TraversalAlgorithmHint::Auto,
            pushdown_predicates: Vec::new(),
        };

        // Analyze plan steps
        for step in &plan.steps {
            match &step.step_type {
                crate::graph::query::planner::PlanStepType::NodeScan {
                    labels,
                    property_filters,
                } => {
                    // Suggest property index if available and selective
                    for filter in property_filters {
                        if let Some(index_stats) = stats.property_indexes.get(&filter.property_name)
                        {
                            if index_stats.exists && index_stats.selectivity < 0.1 {
                                hints.use_property_index = Some(filter.property_name.clone());
                            }
                        }
                    }

                    // Record traversal order (labels indicate starting points)
                    if let Some(labels) = labels {
                        hints.traversal_order.extend(labels.clone());
                    }
                }
                crate::graph::query::planner::PlanStepType::Traverse {
                    algorithm,
                    max_depth,
                    ..
                } => {
                    // Suggest algorithm based on depth and cost
                    match algorithm {
                        crate::graph::query::planner::TraversalAlgorithm::BFS => {
                            hints.suggested_algorithm = TraversalAlgorithmHint::BFS;
                        }
                        crate::graph::query::planner::TraversalAlgorithm::DFS => {
                            hints.suggested_algorithm = TraversalAlgorithmHint::DFS;
                        }
                        crate::graph::query::planner::TraversalAlgorithm::Dijkstra => {
                            hints.suggested_algorithm = TraversalAlgorithmHint::Dijkstra;
                        }
                        crate::graph::query::planner::TraversalAlgorithm::AStar => {
                            hints.suggested_algorithm = TraversalAlgorithmHint::AStar;
                        }
                    }

                    // For deep traversals, suggest DFS
                    if max_depth.unwrap_or(1) > 3 {
                        hints.suggested_algorithm = TraversalAlgorithmHint::DFS;
                    }
                }
                crate::graph::query::planner::PlanStepType::Filter { condition } => {
                    // Suggest pushing down filters to graph engine
                    // TODO: Parse condition and extract pushdown predicates
                    let _ = condition;
                    hints
                        .pushdown_predicates
                        .push("filter_condition".to_string());
                }
                _ => {}
            }
        }

        Ok(hints)
    }

    /// Estimate cost of a specific graph operation
    pub fn estimate_operation_cost(
        &self,
        operation: &GraphOperation,
        stats: &GraphStatistics,
    ) -> GraphOperationCost {
        match operation {
            GraphOperation::NodeScan { labels, filters } => {
                let selectivity = self
                    .estimate_node_selectivity(labels, filters, stats)
                    .unwrap_or(0.5);
                GraphOperationCost {
                    node_scan_cost: stats.node_count as f64 * selectivity,
                    edge_traversal_cost: 0.0,
                    result_count: stats.node_count as f64 * selectivity,
                    total_cost: stats.node_count as f64 * selectivity * 0.1,
                }
            }
            GraphOperation::EdgeTraversal { start_count, depth } => {
                let traversal_cost = *start_count as f64 * stats.avg_fanout * *depth as f64;
                GraphOperationCost {
                    node_scan_cost: 0.0,
                    edge_traversal_cost: traversal_cost,
                    result_count: *start_count as f64 * stats.avg_fanout.powi(*depth as i32),
                    total_cost: traversal_cost,
                }
            }
        }
    }

    /// Clear statistics cache
    pub fn clear_cache(&mut self) {
        self.statistics_cache.clear();
    }
}

/// Graph operation for cost estimation
#[derive(Debug, Clone)]
pub enum GraphOperation {
    /// Node scan operation
    NodeScan {
        labels: Option<Vec<String>>,
        filters: Vec<crate::graph::query::planner::PropertyFilter>,
    },
    /// Edge traversal operation
    EdgeTraversal { start_count: usize, depth: u32 },
}

// Re-export types at module level
pub use crate::core::error::VectorDBError;
