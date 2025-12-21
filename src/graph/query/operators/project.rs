//! Project operator
//!
//! Projects specific columns from tuples (RETURN clause).

use super::{ColumnSpec, PhysicalOperator, QueryValue, ResultTuple, ValueType};
use anyhow::Result;

/// Projection specification
#[derive(Debug, Clone)]
pub struct ProjectionSpec {
    /// Source variable name
    pub source: String,
    /// Optional alias (AS clause)
    pub alias: Option<String>,
    /// Optional property access (e.g., "name" for n.name)
    pub property: Option<String>,
}

/// Project operator
///
/// Projects specific columns from input tuples (implements RETURN clause).
///
/// # Example
///
/// ```ignore
/// // RETURN n.name AS person_name, n.age
/// let projections = vec![
///     ProjectionSpec {
///         source: "n".to_string(),
///         alias: Some("person_name".to_string()),
///         property: Some("name".to_string()),
///     },
///     ProjectionSpec {
///         source: "n".to_string(),
///         alias: None,
///         property: Some("age".to_string()),
///     },
/// ];
///
/// let mut project = ProjectOperator::new(input, projections);
/// ```
pub struct ProjectOperator {
    /// Input operator
    input: Box<dyn PhysicalOperator>,

    /// Projection specifications
    projections: Vec<ProjectionSpec>,

    /// Output schema
    schema: Vec<ColumnSpec>,
}

impl ProjectOperator {
    /// Create new project operator
    pub fn new(input: Box<dyn PhysicalOperator>, projections: Vec<ProjectionSpec>) -> Self {
        // Build output schema
        let schema = projections
            .iter()
            .map(|spec| {
                let name = spec
                    .alias
                    .clone()
                    .unwrap_or_else(|| {
                        if let Some(ref prop) = spec.property {
                            format!("{}.{}", spec.source, prop)
                        } else {
                            spec.source.clone()
                        }
                    });

                ColumnSpec {
                    name,
                    value_type: ValueType::Property, // Default to property
                }
            })
            .collect();

        Self {
            input,
            projections,
            schema,
        }
    }

    /// Extract value from tuple based on projection spec
    fn extract_value(&self, tuple: &ResultTuple, spec: &ProjectionSpec) -> Result<QueryValue> {
        let source_value = tuple
            .get(&spec.source)
            .ok_or_else(|| anyhow::anyhow!("Variable '{}' not found", spec.source))?;

        if let Some(ref prop_name) = spec.property {
            // Extract property from node/edge
            if let Some(node) = source_value.as_node() {
                if let Some(prop_value) = node.properties.get(prop_name) {
                    return Ok(QueryValue::Property(prop_value.clone()));
                } else {
                    return Ok(QueryValue::Null);
                }
            } else if let Some(edge) = source_value.as_edge() {
                if let Some(prop_value) = edge.properties.get(prop_name) {
                    return Ok(QueryValue::Property(prop_value.clone()));
                } else {
                    return Ok(QueryValue::Null);
                }
            }
            Err(anyhow::anyhow!(
                "Cannot extract property from non-node/edge value"
            ))
        } else {
            // Return entire value
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

            // Project each specified column
            for spec in &self.projections {
                let value = self.extract_value(&input_tuple, spec)?;

                let output_name = spec
                    .alias
                    .clone()
                    .unwrap_or_else(|| {
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
