//! Schema Utilities
//!
//! Helpers for schema operations, validation, and evolution.

use std::collections::HashMap;

use anyhow::{Result, anyhow};

use super::types::{
    CatalogColumn, CatalogDataType, CatalogIndex, CatalogIndexType, CatalogSchemaEvolution,
    CatalogTableSchema, ColumnConstraint, SchemaChange,
};

/// Schema builder for creating table schemas
#[derive(Debug, Default)]
pub struct SchemaBuilder {
    name: String,
    columns: Vec<CatalogColumn>,
    primary_key: Vec<String>,
    indexes: Vec<CatalogIndex>,
    properties: HashMap<String, String>,
    next_column_id: i32,
}

impl SchemaBuilder {
    /// Create a new schema builder
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            next_column_id: 1,
            ..Default::default()
        }
    }

    /// Add a column
    pub fn column(mut self, name: impl Into<String>, data_type: CatalogDataType) -> Self {
        let id = self.next_column_id;
        self.next_column_id += 1;
        self.columns.push(CatalogColumn::new(id, name, data_type));
        self
    }

    /// Add a non-nullable column
    pub fn column_not_null(mut self, name: impl Into<String>, data_type: CatalogDataType) -> Self {
        let id = self.next_column_id;
        self.next_column_id += 1;
        self.columns
            .push(CatalogColumn::new(id, name, data_type).nullable(false));
        self
    }

    /// Add a column with default value
    pub fn column_with_default(
        mut self,
        name: impl Into<String>,
        data_type: CatalogDataType,
        default: impl Into<String>,
    ) -> Self {
        let id = self.next_column_id;
        self.next_column_id += 1;
        self.columns
            .push(CatalogColumn::new(id, name, data_type).with_default(default));
        self
    }

    /// Add a vector column
    pub fn vector_column(
        mut self,
        name: impl Into<String>,
        dimension: u32,
        metric: impl Into<String>,
    ) -> Self {
        let id = self.next_column_id;
        self.next_column_id += 1;
        let mut col = CatalogColumn::new(id, name, CatalogDataType::Vector);
        col.properties
            .insert("dimension".to_string(), dimension.to_string());
        col.properties.insert("metric".to_string(), metric.into());
        self.columns.push(col);
        self
    }

    /// Set primary key columns
    pub fn primary_key(mut self, columns: Vec<impl Into<String>>) -> Self {
        self.primary_key = columns.into_iter().map(|c| c.into()).collect();
        self
    }

    /// Add an index
    pub fn index(
        mut self,
        name: impl Into<String>,
        columns: Vec<impl Into<String>>,
        index_type: CatalogIndexType,
    ) -> Self {
        self.indexes.push(CatalogIndex::new(
            name,
            columns.into_iter().map(|c| c.into()).collect(),
            index_type,
        ));
        self
    }

    /// Add a unique index
    pub fn unique_index(
        mut self,
        name: impl Into<String>,
        columns: Vec<impl Into<String>>,
    ) -> Self {
        self.indexes.push(
            CatalogIndex::new(
                name,
                columns.into_iter().map(|c| c.into()).collect(),
                CatalogIndexType::BTree,
            )
            .unique(),
        );
        self
    }

    /// Add a vector index (HNSW)
    pub fn hnsw_index(
        mut self,
        name: impl Into<String>,
        column: impl Into<String>,
        m: u32,
        ef_construction: u32,
    ) -> Self {
        let mut index = CatalogIndex::new(name, vec![column.into()], CatalogIndexType::Hnsw);
        index.properties.insert("m".to_string(), m.to_string());
        index
            .properties
            .insert("ef_construction".to_string(), ef_construction.to_string());
        self.indexes.push(index);
        self
    }

    /// Add a property
    pub fn property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Build the schema
    pub fn build(self) -> CatalogTableSchema {
        CatalogTableSchema {
            name: self.name,
            columns: self.columns,
            primary_key: self.primary_key,
            indexes: self.indexes,
            schema_version: 1,
            properties: self.properties,
            location: None,
            created_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            updated_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        }
    }
}

/// Schema evolution builder
#[derive(Debug, Default)]
pub struct EvolutionBuilder {
    changes: Vec<SchemaChange>,
}

impl EvolutionBuilder {
    /// Create a new evolution builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a column
    pub fn add_column(
        mut self,
        name: impl Into<String>,
        data_type: CatalogDataType,
        nullable: bool,
        default_value: Option<String>,
        comment: Option<String>,
        after: Option<String>,
    ) -> Self {
        self.changes.push(SchemaChange::AddColumn {
            name: name.into(),
            data_type,
            nullable,
            default_value,
            comment,
            after,
        });
        self
    }

    /// Drop a column
    pub fn drop_column(mut self, name: impl Into<String>) -> Self {
        self.changes
            .push(SchemaChange::DropColumn { name: name.into() });
        self
    }

    /// Rename a column
    pub fn rename_column(
        mut self,
        old_name: impl Into<String>,
        new_name: impl Into<String>,
    ) -> Self {
        self.changes.push(SchemaChange::RenameColumn {
            old_name: old_name.into(),
            new_name: new_name.into(),
        });
        self
    }

    /// Change column type
    pub fn change_type(mut self, name: impl Into<String>, new_type: CatalogDataType) -> Self {
        self.changes.push(SchemaChange::ChangeType {
            name: name.into(),
            new_type,
        });
        self
    }

    /// Make column nullable
    pub fn make_nullable(mut self, name: impl Into<String>) -> Self {
        self.changes
            .push(SchemaChange::MakeNullable { name: name.into() });
        self
    }

    /// Set default value
    pub fn set_default(
        mut self,
        name: impl Into<String>,
        default_value: impl Into<String>,
    ) -> Self {
        self.changes.push(SchemaChange::SetDefault {
            name: name.into(),
            default_value: default_value.into(),
        });
        self
    }

    /// Drop default value
    pub fn drop_default(mut self, name: impl Into<String>) -> Self {
        self.changes
            .push(SchemaChange::DropDefault { name: name.into() });
        self
    }

    /// Make column NOT NULL
    pub fn make_not_nullable(mut self, name: impl Into<String>) -> Self {
        self.changes
            .push(SchemaChange::MakeNotNullable { name: name.into() });
        self
    }

    /// Move column to first position
    pub fn move_column_first(mut self, name: impl Into<String>) -> Self {
        self.changes.push(SchemaChange::MoveColumn {
            name: name.into(),
            after: None,
        });
        self
    }

    /// Move column after another column
    pub fn move_column_after(mut self, name: impl Into<String>, after: impl Into<String>) -> Self {
        self.changes.push(SchemaChange::MoveColumn {
            name: name.into(),
            after: Some(after.into()),
        });
        self
    }

    /// Add a UNIQUE constraint
    pub fn add_unique_constraint(
        mut self,
        constraint_name: Option<String>,
        columns: Vec<impl Into<String>>,
    ) -> Self {
        self.changes.push(SchemaChange::AddConstraint {
            constraint_name,
            constraint: ColumnConstraint::Unique {
                columns: columns.into_iter().map(|c| c.into()).collect(),
            },
        });
        self
    }

    /// Add a CHECK constraint
    pub fn add_check_constraint(
        mut self,
        constraint_name: Option<String>,
        expression: impl Into<String>,
    ) -> Self {
        self.changes.push(SchemaChange::AddConstraint {
            constraint_name,
            constraint: ColumnConstraint::Check {
                expression: expression.into(),
            },
        });
        self
    }

    /// Drop a constraint
    pub fn drop_constraint(mut self, constraint_name: impl Into<String>) -> Self {
        self.changes.push(SchemaChange::DropConstraint {
            constraint_name: constraint_name.into(),
        });
        self
    }

    /// Build the evolution request
    pub fn build(self) -> CatalogSchemaEvolution {
        CatalogSchemaEvolution {
            changes: self.changes,
        }
    }
}

/// Validate a schema for correctness
pub fn validate_schema(schema: &CatalogTableSchema) -> Result<()> {
    // Must have a name
    if schema.name.is_empty() {
        return Err(anyhow!("Schema name cannot be empty"));
    }

    // Must have at least one column
    if schema.columns.is_empty() {
        return Err(anyhow!("Schema must have at least one column"));
    }

    // Check for duplicate column names
    let mut seen_columns = std::collections::HashSet::new();
    for col in &schema.columns {
        if col.name.is_empty() {
            return Err(anyhow!("Column name cannot be empty"));
        }
        if !seen_columns.insert(&col.name) {
            return Err(anyhow!("Duplicate column name: {}", col.name));
        }
    }

    // Validate primary key columns exist
    for pk in &schema.primary_key {
        if !schema.columns.iter().any(|c| &c.name == pk) {
            return Err(anyhow!("Primary key column '{}' not found in schema", pk));
        }
    }

    // Validate index columns exist
    for idx in &schema.indexes {
        for col in &idx.columns {
            if !schema.columns.iter().any(|c| &c.name == col) {
                return Err(anyhow!(
                    "Index '{}' references non-existent column '{}'",
                    idx.name,
                    col
                ));
            }
        }
    }

    // Validate vector columns have dimension
    for col in &schema.columns {
        if col.data_type == CatalogDataType::Vector
            || col.data_type == CatalogDataType::SparseVector
        {
            if !col.properties.contains_key("dimension") {
                return Err(anyhow!(
                    "Vector column '{}' must have 'dimension' property",
                    col.name
                ));
            }
        }
    }

    Ok(())
}

/// Apply schema evolution and return new schema
pub fn apply_evolution(
    schema: &CatalogTableSchema,
    evolution: &CatalogSchemaEvolution,
) -> Result<CatalogTableSchema> {
    let mut new_schema = schema.clone();
    new_schema.schema_version += 1;
    new_schema.updated_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Get next column ID
    let mut next_id = new_schema.columns.iter().map(|c| c.id).max().unwrap_or(0) + 1;

    for change in &evolution.changes {
        match change {
            SchemaChange::AddColumn {
                name,
                data_type,
                nullable,
                default_value,
                comment,
                after,
            } => {
                // Check column doesn't already exist
                if new_schema.columns.iter().any(|c| &c.name == name) {
                    return Err(anyhow!("Column '{}' already exists", name));
                }

                let mut col = CatalogColumn::new(next_id, name, *data_type);
                next_id += 1;
                col.nullable = *nullable;
                col.default_value = default_value.clone();
                col.comment = comment.clone();

                // Insert at position or end
                if let Some(after_col) = after {
                    if let Some(pos) = new_schema.columns.iter().position(|c| &c.name == after_col)
                    {
                        new_schema.columns.insert(pos + 1, col);
                    } else {
                        new_schema.columns.push(col);
                    }
                } else {
                    new_schema.columns.push(col);
                }
            }
            SchemaChange::DropColumn { name } => {
                // Check column exists
                let pos = new_schema
                    .columns
                    .iter()
                    .position(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;

                // Check not in primary key
                if new_schema.primary_key.contains(name) {
                    return Err(anyhow!("Cannot drop primary key column '{}'", name));
                }

                new_schema.columns.remove(pos);

                // Remove from any indexes
                for idx in &mut new_schema.indexes {
                    idx.columns.retain(|c| c != name);
                }
                // Remove empty indexes
                new_schema.indexes.retain(|idx| !idx.columns.is_empty());
            }
            SchemaChange::RenameColumn { old_name, new_name } => {
                // Find and rename column
                let col = new_schema
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == old_name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", old_name))?;

                col.name = new_name.clone();

                // Update primary key references
                for pk in &mut new_schema.primary_key {
                    if pk == old_name {
                        *pk = new_name.clone();
                    }
                }

                // Update index references
                for idx in &mut new_schema.indexes {
                    for col_name in &mut idx.columns {
                        if col_name == old_name {
                            *col_name = new_name.clone();
                        }
                    }
                }
            }
            SchemaChange::ChangeType { name, new_type } => {
                let col = new_schema
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;

                // Check compatibility
                if !is_compatible_type_change(col.data_type, *new_type) {
                    return Err(anyhow!(
                        "Cannot change column '{}' from {:?} to {:?}",
                        name,
                        col.data_type,
                        new_type
                    ));
                }

                col.data_type = *new_type;
            }
            SchemaChange::UpdateComment { name, comment } => {
                let col = new_schema
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;

                col.comment = Some(comment.clone());
            }
            SchemaChange::MakeNullable { name } => {
                let col = new_schema
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;

                col.nullable = true;
            }
            SchemaChange::SetDefault {
                name,
                default_value,
            } => {
                let col = new_schema
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;

                col.default_value = Some(default_value.clone());
            }
            SchemaChange::DropDefault { name } => {
                let col = new_schema
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;

                col.default_value = None;
            }
            SchemaChange::MakeNotNullable { name } => {
                let col = new_schema
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;

                // Note: In a real implementation, this would need to verify
                // that no NULL values exist in the column before allowing this change.
                // For now, we just update the schema metadata.
                col.nullable = false;
            }
            SchemaChange::MoveColumn { name, after } => {
                // Find and remove the column
                let pos = new_schema
                    .columns
                    .iter()
                    .position(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;

                let col = new_schema.columns.remove(pos);

                // Insert at new position
                if let Some(after_col) = after {
                    if let Some(after_pos) =
                        new_schema.columns.iter().position(|c| &c.name == after_col)
                    {
                        new_schema.columns.insert(after_pos + 1, col);
                    } else {
                        return Err(anyhow!(
                            "Column '{}' not found for AFTER positioning",
                            after_col
                        ));
                    }
                } else {
                    // FIRST - insert at position 0
                    new_schema.columns.insert(0, col);
                }
            }
            SchemaChange::AddConstraint {
                constraint_name,
                constraint,
            } => {
                // Store constraint in schema properties
                let constraint_key = match &constraint {
                    ColumnConstraint::Unique { columns } => {
                        format!("constraint:unique:{}", columns.join(","))
                    }
                    ColumnConstraint::Check { expression: _ } => {
                        // Hash the expression for a shorter key
                        format!(
                            "constraint:check:{}",
                            constraint_name.as_deref().unwrap_or("unnamed")
                        )
                    }
                    ColumnConstraint::ForeignKey {
                        columns,
                        references_table,
                        ..
                    } => {
                        format!("constraint:fk:{}:{}", columns.join(","), references_table)
                    }
                };

                let constraint_value = serde_json::to_string(&constraint)
                    .map_err(|e| anyhow!("Failed to serialize constraint: {}", e))?;

                new_schema
                    .properties
                    .insert(constraint_key, constraint_value);

                // For unique constraints, also mark in index properties
                if let ColumnConstraint::Unique { columns } = &constraint {
                    // Check that all columns exist
                    for col_name in columns {
                        if !new_schema.columns.iter().any(|c| &c.name == col_name) {
                            return Err(anyhow!(
                                "Column '{}' not found for UNIQUE constraint",
                                col_name
                            ));
                        }
                    }

                    // Create an implicit unique index if constraint has a name
                    if let Some(name) = constraint_name {
                        let unique_index =
                            CatalogIndex::new(name, columns.clone(), CatalogIndexType::BTree)
                                .unique();
                        new_schema.indexes.push(unique_index);
                    }
                }
            }
            SchemaChange::DropConstraint { constraint_name } => {
                // Remove constraint from properties
                let keys_to_remove: Vec<String> = new_schema
                    .properties
                    .keys()
                    .filter(|k| k.starts_with("constraint:") && k.contains(&*constraint_name))
                    .cloned()
                    .collect();

                if keys_to_remove.is_empty() {
                    // Check if it's an index-based constraint
                    let idx_pos = new_schema
                        .indexes
                        .iter()
                        .position(|idx| idx.name == *constraint_name);
                    if let Some(pos) = idx_pos {
                        new_schema.indexes.remove(pos);
                    } else {
                        return Err(anyhow!("Constraint '{}' not found", constraint_name));
                    }
                } else {
                    for key in keys_to_remove {
                        new_schema.properties.remove(&key);
                    }
                    // Also remove any associated index
                    new_schema
                        .indexes
                        .retain(|idx| idx.name != *constraint_name);
                }
            }
        }
    }

    // Validate the new schema
    validate_schema(&new_schema)?;

    Ok(new_schema)
}

/// Check if a type change is compatible (widening only)
pub fn is_compatible_type_change(from: CatalogDataType, to: CatalogDataType) -> bool {
    // Same type is always compatible
    if from == to {
        return true;
    }

    // Integer widening
    matches!(
        (from, to),
        (CatalogDataType::Int32, CatalogDataType::Int64)
            | (CatalogDataType::Int32, CatalogDataType::Float64)
            | (CatalogDataType::Int64, CatalogDataType::Float64)
            | (CatalogDataType::Float32, CatalogDataType::Float64)
            | (CatalogDataType::Int8, CatalogDataType::Int16)
            | (CatalogDataType::Int8, CatalogDataType::Int32)
            | (CatalogDataType::Int8, CatalogDataType::Int64)
            | (CatalogDataType::Int16, CatalogDataType::Int32)
            | (CatalogDataType::Int16, CatalogDataType::Int64)
    )
}

/// Get SQL type name for a data type
pub fn sql_type_name(data_type: CatalogDataType) -> &'static str {
    match data_type {
        CatalogDataType::Boolean => "BOOLEAN",
        CatalogDataType::Int8 => "TINYINT",
        CatalogDataType::Int16 => "SMALLINT",
        CatalogDataType::Int32 => "INTEGER",
        CatalogDataType::Int64 => "BIGINT",
        CatalogDataType::Float32 => "REAL",
        CatalogDataType::Float64 => "DOUBLE PRECISION",
        CatalogDataType::String => "TEXT",
        CatalogDataType::Binary => "BYTEA",
        CatalogDataType::Date => "DATE",
        CatalogDataType::Timestamp => "TIMESTAMP",
        CatalogDataType::TimestampTz => "TIMESTAMP WITH TIME ZONE",
        CatalogDataType::Time => "TIME",
        CatalogDataType::Uuid => "UUID",
        CatalogDataType::Json => "JSONB",
        CatalogDataType::Decimal => "DECIMAL",
        CatalogDataType::Vector => "VECTOR",
        CatalogDataType::SparseVector => "SPARSE_VECTOR",
        CatalogDataType::BinaryVector => "BINARY_VECTOR",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_builder() {
        let schema = SchemaBuilder::new("users")
            .column("id", CatalogDataType::Int64)
            .column_not_null("name", CatalogDataType::String)
            .column_with_default("active", CatalogDataType::Boolean, "true")
            .primary_key(vec!["id"])
            .build();

        assert_eq!(schema.name, "users");
        assert_eq!(schema.columns.len(), 3);
        assert_eq!(schema.primary_key, vec!["id"]);
        assert_eq!(schema.schema_version, 1);
    }

    #[test]
    fn test_schema_builder_vector() {
        let schema = SchemaBuilder::new("embeddings")
            .column("id", CatalogDataType::String)
            .vector_column("embedding", 768, "cosine")
            .hnsw_index("idx_embedding", "embedding", 16, 100)
            .build();

        assert_eq!(schema.columns.len(), 2);
        assert_eq!(schema.indexes.len(), 1);

        let vec_col = &schema.columns[1];
        assert_eq!(
            vec_col.properties.get("dimension"),
            Some(&"768".to_string())
        );
    }

    #[test]
    fn test_validate_schema() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .primary_key(vec!["id"])
            .build();

        assert!(validate_schema(&schema).is_ok());
    }

    #[test]
    fn test_validate_schema_empty_name() {
        let schema = CatalogTableSchema {
            name: String::new(),
            columns: vec![CatalogColumn::new(1, "id", CatalogDataType::Int64)],
            ..Default::default()
        };

        assert!(validate_schema(&schema).is_err());
    }

    #[test]
    fn test_validate_schema_duplicate_columns() {
        let schema = CatalogTableSchema {
            name: "test".to_string(),
            columns: vec![
                CatalogColumn::new(1, "id", CatalogDataType::Int64),
                CatalogColumn::new(2, "id", CatalogDataType::String),
            ],
            ..Default::default()
        };

        assert!(validate_schema(&schema).is_err());
    }

    #[test]
    fn test_apply_evolution_add_column() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .build();

        let evolution = EvolutionBuilder::new()
            .add_column("name", CatalogDataType::String, true, None, None, None)
            .build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert_eq!(new_schema.columns.len(), 2);
        assert_eq!(new_schema.schema_version, 2);
    }

    #[test]
    fn test_apply_evolution_drop_column() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column("name", CatalogDataType::String)
            .primary_key(vec!["id"])
            .build();

        let evolution = EvolutionBuilder::new().drop_column("name").build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert_eq!(new_schema.columns.len(), 1);
    }

    #[test]
    fn test_apply_evolution_rename_column() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column("old_name", CatalogDataType::String)
            .build();

        let evolution = EvolutionBuilder::new()
            .rename_column("old_name", "new_name")
            .build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert!(new_schema.columns.iter().any(|c| c.name == "new_name"));
        assert!(!new_schema.columns.iter().any(|c| c.name == "old_name"));
    }

    #[test]
    fn test_compatible_type_change() {
        assert!(is_compatible_type_change(
            CatalogDataType::Int32,
            CatalogDataType::Int64
        ));
        assert!(is_compatible_type_change(
            CatalogDataType::Float32,
            CatalogDataType::Float64
        ));
        assert!(!is_compatible_type_change(
            CatalogDataType::String,
            CatalogDataType::Int64
        ));
    }

    #[test]
    fn test_sql_type_name() {
        assert_eq!(sql_type_name(CatalogDataType::Int64), "BIGINT");
        assert_eq!(sql_type_name(CatalogDataType::Vector), "VECTOR");
        assert_eq!(sql_type_name(CatalogDataType::Json), "JSONB");
    }

    #[test]
    fn test_apply_evolution_make_not_nullable() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column("name", CatalogDataType::String) // Default is nullable
            .build();

        assert!(schema.columns[1].nullable);

        let evolution = EvolutionBuilder::new().make_not_nullable("name").build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert!(!new_schema.columns[1].nullable);
        assert_eq!(new_schema.schema_version, 2);
    }

    #[test]
    fn test_apply_evolution_make_nullable() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column_not_null("name", CatalogDataType::String)
            .build();

        assert!(!schema.columns[1].nullable);

        let evolution = EvolutionBuilder::new().make_nullable("name").build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert!(new_schema.columns[1].nullable);
    }

    #[test]
    fn test_apply_evolution_move_column_first() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column("name", CatalogDataType::String)
            .column("email", CatalogDataType::String)
            .build();

        assert_eq!(schema.columns[2].name, "email");

        let evolution = EvolutionBuilder::new().move_column_first("email").build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert_eq!(new_schema.columns[0].name, "email");
        assert_eq!(new_schema.columns[1].name, "id");
        assert_eq!(new_schema.columns[2].name, "name");
    }

    #[test]
    fn test_apply_evolution_move_column_after() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column("name", CatalogDataType::String)
            .column("email", CatalogDataType::String)
            .build();

        // Move email after id (position 1)
        let evolution = EvolutionBuilder::new()
            .move_column_after("email", "id")
            .build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert_eq!(new_schema.columns[0].name, "id");
        assert_eq!(new_schema.columns[1].name, "email");
        assert_eq!(new_schema.columns[2].name, "name");
    }

    #[test]
    fn test_apply_evolution_add_unique_constraint() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column("email", CatalogDataType::String)
            .build();

        let evolution = EvolutionBuilder::new()
            .add_unique_constraint(Some("uq_email".to_string()), vec!["email"])
            .build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();

        // Check that the constraint was added as an index
        assert_eq!(new_schema.indexes.len(), 1);
        assert_eq!(new_schema.indexes[0].name, "uq_email");
        assert!(new_schema.indexes[0].is_unique);
        assert_eq!(new_schema.indexes[0].columns, vec!["email"]);

        // Check that constraint is stored in properties
        assert!(
            new_schema
                .properties
                .contains_key("constraint:unique:email")
        );
    }

    #[test]
    fn test_apply_evolution_add_check_constraint() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column("age", CatalogDataType::Int32)
            .build();

        let evolution = EvolutionBuilder::new()
            .add_check_constraint(Some("chk_age".to_string()), "age >= 0 AND age <= 120")
            .build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();

        // Check that the constraint was stored in properties
        assert!(
            new_schema
                .properties
                .contains_key("constraint:check:chk_age")
        );
    }

    #[test]
    fn test_apply_evolution_drop_constraint() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column("email", CatalogDataType::String)
            .unique_index("uq_email", vec!["email"])
            .build();

        assert_eq!(schema.indexes.len(), 1);

        let evolution = EvolutionBuilder::new().drop_constraint("uq_email").build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert_eq!(new_schema.indexes.len(), 0);
    }

    #[test]
    fn test_apply_evolution_set_default() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column("active", CatalogDataType::Boolean)
            .build();

        assert!(schema.columns[1].default_value.is_none());

        let evolution = EvolutionBuilder::new()
            .set_default("active", "true")
            .build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert_eq!(
            new_schema.columns[1].default_value,
            Some("true".to_string())
        );
    }

    #[test]
    fn test_apply_evolution_drop_default() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .column_with_default("active", CatalogDataType::Boolean, "true")
            .build();

        assert_eq!(schema.columns[1].default_value, Some("true".to_string()));

        let evolution = EvolutionBuilder::new().drop_default("active").build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert!(new_schema.columns[1].default_value.is_none());
    }

    #[test]
    fn test_apply_evolution_change_type() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int32)
            .build();

        let evolution = EvolutionBuilder::new()
            .change_type("id", CatalogDataType::Int64)
            .build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();
        assert_eq!(new_schema.columns[0].data_type, CatalogDataType::Int64);
    }

    #[test]
    fn test_apply_evolution_change_type_incompatible() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::String)
            .build();

        let evolution = EvolutionBuilder::new()
            .change_type("id", CatalogDataType::Int64)
            .build();

        // Should fail because String -> Int64 is not compatible
        assert!(apply_evolution(&schema, &evolution).is_err());
    }

    #[test]
    fn test_apply_evolution_multiple_changes() {
        let schema = SchemaBuilder::new("users")
            .column("id", CatalogDataType::Int64)
            .column("name", CatalogDataType::String)
            .column("email", CatalogDataType::String)
            .primary_key(vec!["id"])
            .build();

        let evolution = EvolutionBuilder::new()
            .make_not_nullable("email")
            .add_unique_constraint(Some("uq_email".to_string()), vec!["email"])
            .set_default("name", "'unknown'")
            .build();

        let new_schema = apply_evolution(&schema, &evolution).unwrap();

        // Check all changes were applied
        let email_col = new_schema
            .columns
            .iter()
            .find(|c| c.name == "email")
            .unwrap();
        assert!(!email_col.nullable);

        let name_col = new_schema
            .columns
            .iter()
            .find(|c| c.name == "name")
            .unwrap();
        assert_eq!(name_col.default_value, Some("'unknown'".to_string()));

        assert_eq!(new_schema.indexes.len(), 1);
        assert!(new_schema.indexes[0].is_unique);
    }

    #[test]
    fn test_apply_evolution_column_not_found() {
        let schema = SchemaBuilder::new("test")
            .column("id", CatalogDataType::Int64)
            .build();

        let evolution = EvolutionBuilder::new()
            .make_not_nullable("nonexistent")
            .build();

        // Should fail because column doesn't exist
        assert!(apply_evolution(&schema, &evolution).is_err());
    }
}
