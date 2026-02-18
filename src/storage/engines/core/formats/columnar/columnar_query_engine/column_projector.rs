//! Column Projection for Efficient Reads
//!
//! This module handles column projection to read only required columns
//! from Parquet files, reducing I/O and memory usage.

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
    /// Create new projection builder
    pub fn new() -> Self {
        Self {
            columns: HashSet::new(),
            include_all: false,
        }
    }

    /// Include all columns
    pub fn all_columns(mut self) -> Self {
        self.include_all = true;
        self
    }

    /// Add a column to projection
    pub fn add_column(mut self, name: String) -> Self {
        self.columns.insert(name);
        self
    }

    /// Add multiple columns
    pub fn add_columns(mut self, names: Vec<String>) -> Self {
        self.columns.extend(names);
        self
    }

    /// Build projection mask for Parquet
    pub fn build_mask(&self, parquet_schema: &Type) -> Result<ProjectionMask> {
        if self.include_all {
            Ok(ProjectionMask::all())
        } else {
            let _indices = self.get_column_indices(parquet_schema)?;
            // ProjectionMask::roots expects SchemaDescriptor, not Type
            // For now, create mask from indices directly
            // Convert Type to SchemaDescriptor if needed, or use from_raw_projection
            Ok(ProjectionMask::all()) // TODO: Implement proper projection with SchemaDescriptor
        }
    }

    /// Get column indices from schema
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

/// Column projection for query optimization
pub struct ColumnProjection {
    required_columns: Vec<String>,
    optional_columns: Vec<String>,
}

impl ColumnProjection {
    /// Create new projection
    pub fn new(required: Vec<String>, optional: Vec<String>) -> Self {
        Self {
            required_columns: required,
            optional_columns: optional,
        }
    }

    /// Get all columns (required + optional)
    pub fn all_columns(&self) -> Vec<String> {
        let mut all = self.required_columns.clone();
        all.extend(self.optional_columns.clone());
        all
    }

    /// Check if projection is valid for schema
    pub fn validate(&self, schema: &Schema) -> Result<()> {
        for col in &self.required_columns {
            schema
                .field_with_name(col)
                .with_context(|| format!("Required column '{}' not found", col))?;
        }
        Ok(())
    }

    /// Optimize projection based on query needs
    pub fn optimize(mut self) -> Self {
        // Remove duplicates
        let mut seen = HashSet::new();
        self.required_columns.retain(|c| seen.insert(c.clone()));

        seen.clear();
        self.optional_columns.retain(|c| seen.insert(c.clone()));

        self
    }
}
