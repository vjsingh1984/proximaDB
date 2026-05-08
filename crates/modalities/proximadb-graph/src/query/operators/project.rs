use super::{ColumnSpec, PhysicalOperator, QueryValue, ResultTuple, ValueType};
use anyhow::Result;

/// Projection specification.
#[derive(Debug, Clone)]
pub struct ProjectionSpec {
    pub source: String,
    pub alias: Option<String>,
    pub property: Option<String>,
}

/// Project operator.
pub struct ProjectOperator {
    input: Box<dyn PhysicalOperator>,
    projections: Vec<ProjectionSpec>,
    schema: Vec<ColumnSpec>,
}

impl ProjectOperator {
    pub fn new(input: Box<dyn PhysicalOperator>, projections: Vec<ProjectionSpec>) -> Self {
        let schema = projections
            .iter()
            .map(|spec| {
                let name = spec.alias.clone().unwrap_or_else(|| {
                    if let Some(ref prop) = spec.property {
                        format!("{}.{}", spec.source, prop)
                    } else {
                        spec.source.clone()
                    }
                });
                ColumnSpec {
                    name,
                    value_type: ValueType::Property,
                }
            })
            .collect();

        Self {
            input,
            projections,
            schema,
        }
    }

    fn extract_value(&self, tuple: &ResultTuple, spec: &ProjectionSpec) -> Result<QueryValue> {
        let source_value = tuple
            .get(&spec.source)
            .ok_or_else(|| anyhow::anyhow!("Variable '{}' not found", spec.source))?;

        if let Some(ref prop_name) = spec.property {
            if let Some(node) = source_value.as_node() {
                return Ok(node
                    .properties
                    .get(prop_name)
                    .cloned()
                    .map(QueryValue::Property)
                    .unwrap_or(QueryValue::Null));
            }
            if let Some(edge) = source_value.as_edge() {
                return Ok(edge
                    .properties
                    .get(prop_name)
                    .cloned()
                    .map(QueryValue::Property)
                    .unwrap_or(QueryValue::Null));
            }
            Err(anyhow::anyhow!(
                "Cannot extract property from non-node/edge value"
            ))
        } else {
            Ok(source_value.clone())
        }
    }
}

impl PhysicalOperator for ProjectOperator {
    fn open(&mut self) -> Result<()> {
        self.input.open()
    }

    fn next(&mut self) -> Result<Option<ResultTuple>> {
        if let Some(input_tuple) = self.input.next()? {
            let mut output_tuple = ResultTuple::new();
            for spec in &self.projections {
                let value = self.extract_value(&input_tuple, spec)?;
                let output_name = spec.alias.clone().unwrap_or_else(|| {
                    if let Some(ref prop) = spec.property {
                        format!("{}.{}", spec.source, prop)
                    } else {
                        spec.source.clone()
                    }
                });
                output_tuple.set(output_name, value);
            }
            Ok(Some(output_tuple))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> Result<()> {
        self.input.close()
    }

    fn estimated_cardinality(&self) -> usize {
        self.input.estimated_cardinality()
    }

    fn schema(&self) -> &[ColumnSpec] {
        &self.schema
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_proto::proximadb_v1::{Edge, Node, PropertyValue, property_value::Value};
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

    fn int_property(value: i64) -> PropertyValue {
        PropertyValue {
            value: Some(Value::IntValue(value)),
        }
    }

    fn string_property(value: &str) -> PropertyValue {
        PropertyValue {
            value: Some(Value::StringValue(value.to_string())),
        }
    }

    #[test]
    fn project_operator_extracts_node_and_edge_properties() {
        let mut tuple = ResultTuple::new();
        let node = Arc::new(Node {
            id: "n1".to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::from([
                ("name".to_string(), string_property("Alice")),
                ("age".to_string(), int_property(30)),
            ]),
            ..Default::default()
        });
        let edge = Arc::new(Edge {
            id: "e1".to_string(),
            from_node_id: "n1".to_string(),
            to_node_id: "n2".to_string(),
            edge_type: "KNOWS".to_string(),
            properties: HashMap::from([("since".to_string(), int_property(2020))]),
            ..Default::default()
        });
        tuple.set("n".to_string(), QueryValue::Node(node));
        tuple.set("r".to_string(), QueryValue::Edge(edge));

        let input = MockInputOperator::new(
            vec![tuple],
            vec![ColumnSpec {
                name: "n".to_string(),
                value_type: ValueType::Node,
            }],
        );
        let projections = vec![
            ProjectionSpec {
                source: "n".to_string(),
                alias: Some("person_name".to_string()),
                property: Some("name".to_string()),
            },
            ProjectionSpec {
                source: "r".to_string(),
                alias: None,
                property: Some("since".to_string()),
            },
        ];

        let mut project = ProjectOperator::new(Box::new(input), projections);
        project.open().unwrap();
        let result = project.next().unwrap().expect("expected projected tuple");
        assert_eq!(
            result.get("person_name").and_then(QueryValue::as_property),
            Some(&string_property("Alice"))
        );
        assert_eq!(
            result.get("r.since").and_then(QueryValue::as_property),
            Some(&int_property(2020))
        );
    }
}
