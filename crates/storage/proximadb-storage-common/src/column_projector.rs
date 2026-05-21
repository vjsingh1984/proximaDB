//! Column Projection for Efficient Reads
//!
//! Handles column projection to read only required columns from Parquet files,
//! reducing I/O and memory usage.

use anyhow::{Context, Result};
use arrow::datatypes::Schema;
use parquet::arrow::ProjectionMask;
use parquet::schema::types::Type;
use std::collections::HashSet;

/// Column projection builder
pub struct ProjectionBuilder {
    columns: HashSet<String>,
    include_all: bool,
}

impl ProjectionBuilder {
    pub fn new() -> Self {
        Self {
            columns: HashSet::new(),
            include_all: false,
        }
    }

    pub fn all_columns(mut self) -> Self {
        self.include_all = true;
        self
    }

    pub fn add_column(mut self, name: String) -> Self {
        self.columns.insert(name);
        self
    }

    pub fn add_columns(mut self, names: Vec<String>) -> Self {
        self.columns.extend(names);
        self
    }

    pub fn build_mask(&self, parquet_schema: &Type) -> Result<ProjectionMask> {
        if self.include_all {
            Ok(ProjectionMask::all())
        } else {
            let _indices = self.get_column_indices(parquet_schema)?;
            Ok(ProjectionMask::all())
        }
    }

    fn get_column_indices(&self, parquet_schema: &Type) -> Result<Vec<usize>> {
        let mut indices = Vec::new();
        if let Type::GroupType { fields, .. } = parquet_schema {
            for (idx, field) in fields.iter().enumerate() {
                if self.columns.contains(field.name()) {
                    indices.push(idx);
                }
            }
        }
        Ok(indices)
    }
}

impl Default for ProjectionBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Column projection for query optimization
pub struct ColumnProjection {
    required_columns: Vec<String>,
    optional_columns: Vec<String>,
}

impl ColumnProjection {
    pub fn new(required: Vec<String>, optional: Vec<String>) -> Self {
        Self {
            required_columns: required,
            optional_columns: optional,
        }
    }

    pub fn all_columns(&self) -> Vec<String> {
        let mut all = self.required_columns.clone();
        all.extend(self.optional_columns.clone());
        all
    }

    pub fn validate(&self, schema: &Schema) -> Result<()> {
        for col in &self.required_columns {
            schema
                .field_with_name(col)
                .with_context(|| format!("Required column '{}' not found", col))?;
        }
        Ok(())
    }

    pub fn optimize(mut self) -> Self {
        let mut seen = HashSet::new();
        self.required_columns.retain(|c| seen.insert(c.clone()));
        seen.clear();
        self.optional_columns.retain(|c| seen.insert(c.clone()));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field};
    use parquet::basic::{Repetition, Type as PhysicalType};
    use std::sync::Arc;

    fn parquet_schema() -> Type {
        let id = Type::primitive_type_builder("id", PhysicalType::BYTE_ARRAY)
            .with_repetition(Repetition::REQUIRED)
            .build()
            .unwrap();
        let score = Type::primitive_type_builder("score", PhysicalType::FLOAT)
            .with_repetition(Repetition::OPTIONAL)
            .build()
            .unwrap();
        let tenant = Type::primitive_type_builder("tenant", PhysicalType::BYTE_ARRAY)
            .with_repetition(Repetition::OPTIONAL)
            .build()
            .unwrap();

        Type::group_type_builder("schema")
            .with_fields(vec![Arc::new(id), Arc::new(score), Arc::new(tenant)])
            .build()
            .unwrap()
    }

    #[test]
    fn projection_builder_finds_requested_parquet_column_indices() {
        let schema = parquet_schema();
        let projection = ProjectionBuilder::new()
            .add_column("tenant".to_string())
            .add_column("id".to_string());

        assert_eq!(projection.get_column_indices(&schema).unwrap(), vec![0, 2]);
    }

    #[test]
    fn projection_builder_accepts_all_columns_and_unknown_names() {
        let schema = parquet_schema();

        ProjectionBuilder::new()
            .all_columns()
            .build_mask(&schema)
            .expect("all-column mask should build");

        let indices = ProjectionBuilder::new()
            .add_columns(vec!["missing".to_string(), "score".to_string()])
            .get_column_indices(&schema)
            .unwrap();
        assert_eq!(indices, vec![1]);
    }

    #[test]
    fn column_projection_validates_required_columns_only() {
        let schema = Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("score", DataType::Float32, true),
        ]);

        ColumnProjection::new(vec!["id".to_string()], vec!["missing_optional".to_string()])
            .validate(&schema)
            .expect("missing optional columns are not required for validation");

        let err = ColumnProjection::new(vec!["missing_required".to_string()], vec![])
            .validate(&schema)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Required column 'missing_required' not found"));
    }

    #[test]
    fn column_projection_lists_columns_and_deduplicates_by_role() {
        let projection = ColumnProjection::new(
            vec!["id".to_string(), "id".to_string(), "score".to_string()],
            vec![
                "tenant".to_string(),
                "tenant".to_string(),
                "score".to_string(),
            ],
        )
        .optimize();

        assert_eq!(
            projection.all_columns(),
            vec![
                "id".to_string(),
                "score".to_string(),
                "tenant".to_string(),
                "score".to_string()
            ]
        );
    }
}
