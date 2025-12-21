//! Filter operator
//!
//! Filters tuples based on predicates (WHERE clause).

use super::{ColumnSpec, PhysicalOperator, QueryValue, ResultTuple, ValueType};
use anyhow::Result;
use std::sync::Arc;

/// Filter expression for WHERE clause
#[derive(Debug, Clone)]
pub enum FilterExpression {
    /// Property comparison (e.g., n.age > 25)
    Property {
        variable: String,
        property: String,
        operator: ComparisonOperator,
        value: FilterValue,
    },
    /// Logical AND
    And(Box<FilterExpression>, Box<FilterExpression>),
    /// Logical OR
    Or(Box<FilterExpression>, Box<FilterExpression>),
    /// Logical NOT
    Not(Box<FilterExpression>),
    /// Always true
    True,
}

/// Comparison operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonOperator {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    GreaterEqual,
    LessEqual,
    Contains,
    StartsWith,
    EndsWith,
}

/// Filter value
#[derive(Debug, Clone)]
pub enum FilterValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// Filter operator
///
/// Filters tuples based on WHERE clause predicates.
///
/// # Design
///
/// - **Streaming**: Evaluates predicates on-the-fly, no materialization
/// - **Composable**: Can be chained with other operators
/// - **Efficient**: Short-circuit evaluation for AND/OR
///
/// # Example
///
/// ```ignore
/// // Filter Person nodes with age > 25
/// let filter_expr = FilterExpression::Property {
///     variable: "p".to_string(),
///     property: "age".to_string(),
///     operator: ComparisonOperator::GreaterThan,
///     value: FilterValue::Int(25),
/// };
///
/// let mut filter = FilterOperator::new(scan_operator, filter_expr);
/// filter.open()?;
/// while let Some(tuple) = filter.next()? {
///     // Only tuples satisfying age > 25
/// }
/// ```
pub struct FilterOperator {
    /// Input operator
    input: Box<dyn PhysicalOperator>,

    /// Filter condition
    condition: FilterExpression,

    /// Pass-through cardinality estimate
    estimated_cardinality: usize,
}

impl FilterOperator {
    /// Create new filter operator
    pub fn new(input: Box<dyn PhysicalOperator>, condition: FilterExpression) -> Self {
        Self {
            input,
            condition,
            estimated_cardinality: 0,
        }
    }

    /// Evaluate filter condition against tuple
    fn evaluate_condition(&self, tuple: &ResultTuple) -> Result<bool> {
        match &self.condition {
            FilterExpression::Property {
                variable,
                property,
                operator,
                value,
            } => {
                // Get node from tuple
                let query_value = tuple
                    .get(variable)
                    .ok_or_else(|| anyhow::anyhow!("Variable '{}' not found in tuple", variable))?;

                let node = query_value
                    .as_node()
                    .ok_or_else(|| anyhow::anyhow!("Variable '{}' is not a node", variable))?;

                // Get property value
                let prop_value = node.properties.get(property);

                if let Some(actual) = prop_value {
                    Ok(Self::compare_values(actual, operator, value))
                } else {
                    // Property doesn't exist
                    Ok(false)
                }
            }
            FilterExpression::And(left, right) => {
                // Short-circuit evaluation
                let left_result = self.evaluate_expression(left, tuple)?;
                if !left_result {
                    return Ok(false);
                }
                self.evaluate_expression(right, tuple)
            }
            FilterExpression::Or(left, right) => {
                // Short-circuit evaluation
                let left_result = self.evaluate_expression(left, tuple)?;
                if left_result {
                    return Ok(true);
                }
                self.evaluate_expression(right, tuple)
            }
            FilterExpression::Not(inner) => {
                let result = self.evaluate_expression(inner, tuple)?;
                Ok(!result)
            }
            FilterExpression::True => Ok(true),
        }
    }

    /// Helper to evaluate nested expressions (recursively)
    fn evaluate_expression(&self, expr: &FilterExpression, tuple: &ResultTuple) -> Result<bool> {
        match expr {
            FilterExpression::Property { variable, property, operator, value } => {
                let query_value = tuple.get(variable).ok_or_else(|| anyhow::anyhow!("Variable not found"))?;
                let node = query_value.as_node().ok_or_else(|| anyhow::anyhow!("Not a node"))?;
                let prop_value = node.properties.get(property);

                if let Some(actual) = prop_value {
                    Ok(Self::compare_values(actual, operator, value))
                } else {
                    Ok(false)
                }
            }
            FilterExpression::And(left, right) => {
                let left_result = self.evaluate_expression(left, tuple)?;
                if !left_result {
                    return Ok(false);
                }
                self.evaluate_expression(right, tuple)
            }
            FilterExpression::Or(left, right) => {
                let left_result = self.evaluate_expression(left, tuple)?;
                if left_result {
                    return Ok(true);
                }
                self.evaluate_expression(right, tuple)
            }
            FilterExpression::Not(inner) => {
                let result = self.evaluate_expression(inner, tuple)?;
                Ok(!result)
            }
            FilterExpression::True => Ok(true),
        }
    }

    /// Compare property value against filter value
    fn compare_values(
        actual: &crate::proto::proximadb_v1::PropertyValue,
        operator: &ComparisonOperator,
        expected: &FilterValue,
    ) -> bool {
        use crate::proto::proximadb_v1::property_value::Value;

        match (&actual.value, expected) {
            (Some(Value::IntValue(a)), FilterValue::Int(b)) => match operator {
                ComparisonOperator::Equals => a == b,
                ComparisonOperator::NotEquals => a != b,
                ComparisonOperator::GreaterThan => a > b,
                ComparisonOperator::LessThan => a < b,
                ComparisonOperator::GreaterEqual => a >= b,
                ComparisonOperator::LessEqual => a <= b,
                _ => false,
            },
            (Some(Value::StringValue(a)), FilterValue::String(b)) => match operator {
                ComparisonOperator::Equals => a == b,
                ComparisonOperator::NotEquals => a != b,
                ComparisonOperator::Contains => a.contains(b),
                ComparisonOperator::StartsWith => a.starts_with(b),
                ComparisonOperator::EndsWith => a.ends_with(b),
                _ => false,
            },
            (Some(Value::DoubleValue(a)), FilterValue::Float(b)) => match operator {
                ComparisonOperator::Equals => (a - b).abs() < f64::EPSILON,
                ComparisonOperator::NotEquals => (a - b).abs() >= f64::EPSILON,
                ComparisonOperator::GreaterThan => a > b,
                ComparisonOperator::LessThan => a < b,
                ComparisonOperator::GreaterEqual => a >= b,
                ComparisonOperator::LessEqual => a <= b,
                _ => false,
            },
            _ => false,
        }
    }
}

impl PhysicalOperator for FilterOperator {
    fn open(&mut self) -> Result<()> {
        self.input.open()?;

        // Estimate: assume 50% selectivity (conservative)
        self.estimated_cardinality = self.input.estimated_cardinality() / 2;

        Ok(())
    }

    fn next(&mut self) -> Result<Option<ResultTuple>> {
        // Keep pulling tuples until we find one that passes the filter
        while let Some(tuple) = self.input.next()? {
            if self.evaluate_condition(&tuple)? {
                return Ok(Some(tuple));
            }
        }
        Ok(None)
    }

    fn close(&mut self) -> Result<()> {
        self.input.close()
    }

    fn estimated_cardinality(&self) -> usize {
        self.estimated_cardinality
    }

    fn schema(&self) -> &[ColumnSpec] {
        // Pass through input schema
        self.input.schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::GraphEngine;
    use crate::graph::query::operators::scan::NodeScanOperator;
    use crate::proto::proximadb_v1::{property_value::Value, Node, PropertyValue};
    use async_trait::async_trait;
    use std::collections::HashMap;

    struct MockEngine {
        nodes: Vec<Arc<Node>>,
    }

    impl MockEngine {
        fn new(nodes: Vec<Node>) -> Self {
            Self {
                nodes: nodes.into_iter().map(Arc::new).collect(),
            }
        }
    }

    #[async_trait]
    impl GraphEngine for MockEngine {
        fn get_nodes_by_label(&self, _label: &str) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(self.nodes.clone())
        }

        fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> {
            Ok(self.nodes.clone())
        }

        // Stub implementations
        async fn insert_node(&self, node: Node) -> Result<Arc<Node>, crate::core::error::ProximaDBError> { Ok(Arc::new(node)) }
        fn get_node(&self, _id: &String) -> Result<Option<Arc<Node>>, crate::core::error::ProximaDBError> { Ok(None) }
        async fn update_node(&self, node: Node) -> Result<Arc<Node>, crate::core::error::ProximaDBError> { Ok(Arc::new(node)) }
        async fn delete_node(&self, _id: &String) -> Result<Option<Arc<Node>>, crate::core::error::ProximaDBError> { Ok(None) }
        async fn insert_edge(&self, edge: crate::proto::proximadb_v1::Edge) -> Result<Arc<crate::proto::proximadb_v1::Edge>, crate::core::error::ProximaDBError> { Ok(Arc::new(edge)) }
        fn get_edge(&self, _id: &String) -> Result<Option<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> { Ok(None) }
        async fn update_edge(&self, edge: crate::proto::proximadb_v1::Edge) -> Result<Arc<crate::proto::proximadb_v1::Edge>, crate::core::error::ProximaDBError> { Ok(Arc::new(edge)) }
        async fn delete_edge(&self, _id: &String) -> Result<Option<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> { Ok(None) }
        fn get_neighbors(&self, _node_id: &String, _edge_type: Option<&str>) -> Result<Vec<Arc<Node>>, crate::core::error::ProximaDBError> { Ok(vec![]) }
        fn get_outgoing_edges(&self, _node_id: &String, _edge_type: Option<&str>) -> Result<Vec<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> { Ok(vec![]) }
        fn get_incoming_edges(&self, _node_id: &String, _edge_type: Option<&str>) -> Result<Vec<Arc<crate::proto::proximadb_v1::Edge>>, crate::core::error::ProximaDBError> { Ok(vec![]) }
        fn node_count(&self) -> Result<usize, crate::core::error::ProximaDBError> { Ok(self.nodes.len()) }
        fn edge_count(&self) -> Result<usize, crate::core::error::ProximaDBError> { Ok(0) }
    }

    fn create_test_node(id: &str, age: i64) -> Node {
        let mut properties = HashMap::new();
        properties.insert(
            "age".to_string(),
            PropertyValue {
                value: Some(Value::IntValue(age)),
            },
        );

        Node {
            id: id.to_string(),
            labels: vec!["Person".to_string()],
            properties,
            ..Default::default()
        }
    }

    #[test]
    fn test_filter_greater_than() {
        let nodes = vec![
            create_test_node("n1", 30),
            create_test_node("n2", 20),
            create_test_node("n3", 40),
        ];

        let engine = Arc::new(MockEngine::new(nodes));
        let scan = NodeScanOperator::new(engine, None, vec![], "p".to_string());

        let filter_expr = FilterExpression::Property {
            variable: "p".to_string(),
            property: "age".to_string(),
            operator: ComparisonOperator::GreaterThan,
            value: FilterValue::Int(25),
        };

        let mut filter = FilterOperator::new(Box::new(scan), filter_expr);
        filter.open().unwrap();

        let mut count = 0;
        while let Some(tuple) = filter.next().unwrap() {
            let node = tuple.get("p").unwrap().as_node().unwrap();
            if let Some(age_val) = node.properties.get("age") {
                if let Some(Value::IntValue(age)) = &age_val.value {
                    assert!(*age > 25);
                }
            }
            count += 1;
        }

        assert_eq!(count, 2); // n1 (30) and n3 (40)

        filter.close().unwrap();
    }

    #[test]
    fn test_filter_and_condition() {
        let nodes = vec![
            create_test_node("n1", 30),
            create_test_node("n2", 20),
            create_test_node("n3", 40),
        ];

        let engine = Arc::new(MockEngine::new(nodes));
        let scan = NodeScanOperator::new(engine, None, vec![], "p".to_string());

        // age > 25 AND age < 35
        let filter_expr = FilterExpression::And(
            Box::new(FilterExpression::Property {
                variable: "p".to_string(),
                property: "age".to_string(),
                operator: ComparisonOperator::GreaterThan,
                value: FilterValue::Int(25),
            }),
            Box::new(FilterExpression::Property {
                variable: "p".to_string(),
                property: "age".to_string(),
                operator: ComparisonOperator::LessThan,
                value: FilterValue::Int(35),
            }),
        );

        let mut filter = FilterOperator::new(Box::new(scan), filter_expr);
        filter.open().unwrap();

        let mut count = 0;
        while filter.next().unwrap().is_some() {
            count += 1;
        }

        assert_eq!(count, 1); // Only n1 (30)

        filter.close().unwrap();
    }
}
