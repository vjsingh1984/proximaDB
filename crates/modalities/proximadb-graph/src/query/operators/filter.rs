use super::{ColumnSpec, PhysicalOperator, ResultTuple};
use anyhow::Result;
use proximadb_proto::proximadb_v1::{PropertyValue, property_value::Value};

/// Filter expression for WHERE clause evaluation.
#[derive(Debug, Clone)]
pub enum FilterExpression {
    Property {
        variable: String,
        property: String,
        operator: ComparisonOperator,
        value: FilterValue,
    },
    And(Box<FilterExpression>, Box<FilterExpression>),
    Or(Box<FilterExpression>, Box<FilterExpression>),
    Not(Box<FilterExpression>),
    True,
}

/// Comparison operators for property-based filtering.
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

/// Filter value representing a typed literal for comparison.
#[derive(Debug, Clone)]
pub enum FilterValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// Filter operator.
pub struct FilterOperator {
    input: Box<dyn PhysicalOperator>,
    condition: FilterExpression,
    estimated_cardinality: usize,
}

impl FilterOperator {
    pub fn new(input: Box<dyn PhysicalOperator>, condition: FilterExpression) -> Self {
        Self {
            input,
            condition,
            estimated_cardinality: 0,
        }
    }

    fn evaluate_condition(&self, tuple: &ResultTuple) -> Result<bool> {
        self.evaluate_expression(&self.condition, tuple)
    }

    fn evaluate_expression(&self, expr: &FilterExpression, tuple: &ResultTuple) -> Result<bool> {
        match expr {
            FilterExpression::Property {
                variable,
                property,
                operator,
                value,
            } => {
                let query_value = tuple
                    .get(variable)
                    .ok_or_else(|| anyhow::anyhow!("Variable '{}' not found in tuple", variable))?;
                let node = query_value
                    .as_node()
                    .ok_or_else(|| anyhow::anyhow!("Variable '{}' is not a node", variable))?;

                Ok(node
                    .properties
                    .get(property)
                    .is_some_and(|actual| Self::compare_values(actual, operator, value)))
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
            FilterExpression::Not(inner) => Ok(!self.evaluate_expression(inner, tuple)?),
            FilterExpression::True => Ok(true),
        }
    }

    fn compare_values(
        actual: &PropertyValue,
        operator: &ComparisonOperator,
        expected: &FilterValue,
    ) -> bool {
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
            (Some(Value::BoolValue(a)), FilterValue::Bool(b)) => match operator {
                ComparisonOperator::Equals => a == b,
                ComparisonOperator::NotEquals => a != b,
                _ => false,
            },
            _ => false,
        }
    }
}

impl PhysicalOperator for FilterOperator {
    fn open(&mut self) -> Result<()> {
        self.input.open()?;
        self.estimated_cardinality = self.input.estimated_cardinality() / 2;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<ResultTuple>> {
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
        self.input.schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::execution::{QueryValue, ResultTuple, ValueType};
    use proximadb_proto::proximadb_v1::Node;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct MockInputOperator {
        tuples: Vec<ResultTuple>,
        index: usize,
        schema: Vec<ColumnSpec>,
    }

    impl MockInputOperator {
        fn new(tuples: Vec<ResultTuple>, schema: Vec<ColumnSpec>) -> Self {
            Self {
                tuples,
                index: 0,
                schema,
            }
        }
    }

    impl PhysicalOperator for MockInputOperator {
        fn open(&mut self) -> Result<()> {
            self.index = 0;
            Ok(())
        }

        fn next(&mut self) -> Result<Option<ResultTuple>> {
            if let Some(tuple) = self.tuples.get(self.index).cloned() {
                self.index += 1;
                Ok(Some(tuple))
            } else {
                Ok(None)
            }
        }

        fn close(&mut self) -> Result<()> {
            Ok(())
        }

        fn estimated_cardinality(&self) -> usize {
            self.tuples.len()
        }

        fn schema(&self) -> &[ColumnSpec] {
            &self.schema
        }
    }

    fn create_test_tuple(id: &str, age: i64, name: &str) -> ResultTuple {
        let mut properties = HashMap::new();
        properties.insert(
            "age".to_string(),
            PropertyValue {
                value: Some(Value::IntValue(age)),
            },
        );
        properties.insert(
            "name".to_string(),
            PropertyValue {
                value: Some(Value::StringValue(name.to_string())),
            },
        );

        let node = Arc::new(Node {
            id: id.to_string(),
            labels: vec!["Person".to_string()],
            properties,
            ..Default::default()
        });
        let mut tuple = ResultTuple::new();
        tuple.set("p".to_string(), QueryValue::Node(node));
        tuple
    }

    #[test]
    fn filter_operator_applies_numeric_and_string_predicates() {
        let input = MockInputOperator::new(
            vec![
                create_test_tuple("n1", 30, "Alice Graph"),
                create_test_tuple("n2", 20, "Bob Docs"),
            ],
            vec![ColumnSpec {
                name: "p".to_string(),
                value_type: ValueType::Node,
            }],
        );
        let filter_expr = FilterExpression::And(
            Box::new(FilterExpression::Property {
                variable: "p".to_string(),
                property: "age".to_string(),
                operator: ComparisonOperator::GreaterThan,
                value: FilterValue::Int(25),
            }),
            Box::new(FilterExpression::Property {
                variable: "p".to_string(),
                property: "name".to_string(),
                operator: ComparisonOperator::Contains,
                value: FilterValue::String("Graph".to_string()),
            }),
        );

        let mut filter = FilterOperator::new(Box::new(input), filter_expr);
        filter.open().unwrap();
        let tuple = filter.next().unwrap().expect("expected matching tuple");
        assert_eq!(tuple.get("p").unwrap().as_node().unwrap().id, "n1");
        assert!(filter.next().unwrap().is_none());
    }
}
