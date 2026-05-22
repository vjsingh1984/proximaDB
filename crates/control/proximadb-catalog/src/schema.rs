// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Schema validation and evolution helpers for xCatalog.
//!
//! All types used here are defined in this crate (`proximadb-catalog`) so this
//! module can be consumed without depending on the root `proximadb` crate.

use std::collections::HashSet;

use anyhow::{Result, anyhow};

use crate::{
    CatalogColumn, CatalogDataType, CatalogIndex, CatalogIndexType, CatalogSchemaEvolution,
    CatalogTableSchema, ColumnConstraint, SchemaChange, system_columns,
};

/// Validate a schema for internal consistency.
pub fn validate_schema(schema: &CatalogTableSchema) -> Result<()> {
    if schema.name.is_empty() {
        return Err(anyhow!("Schema name cannot be empty"));
    }

    if schema.columns.is_empty() {
        return Err(anyhow!("Schema must have at least one column"));
    }

    let mut seen = HashSet::new();
    for col in &schema.columns {
        if col.name.is_empty() {
            return Err(anyhow!("Column name cannot be empty"));
        }
        if system_columns::is_reserved_column_name(&col.name) {
            return Err(anyhow!(
                "Column name '{}' is reserved for ProximaDB system metadata",
                col.name
            ));
        }
        if !seen.insert(&col.name) {
            return Err(anyhow!("Duplicate column name: {}", col.name));
        }
    }

    for pk in &schema.primary_key {
        if !schema.columns.iter().any(|c| &c.name == pk) {
            return Err(anyhow!("Primary key column '{}' not found in schema", pk));
        }
    }

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

    for col in &schema.columns {
        if (col.data_type == CatalogDataType::Vector
            || col.data_type == CatalogDataType::SparseVector)
            && !col.properties.contains_key("dimension")
        {
            return Err(anyhow!(
                "Vector column '{}' must have 'dimension' property",
                col.name
            ));
        }
    }

    validate_storage_contract(schema)?;

    Ok(())
}

fn validate_storage_contract(schema: &CatalogTableSchema) -> Result<()> {
    for layout in &schema.storage_layouts {
        if layout.requires_external_contract() {
            if layout.location.as_deref().unwrap_or_default().is_empty() {
                return Err(anyhow!(
                    "External layout '{}' for table '{}' must declare a location",
                    layout.name,
                    schema.name
                ));
            }
            if layout
                .snapshot_semantics
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            {
                return Err(anyhow!(
                    "External layout '{}' for table '{}' must declare snapshot semantics",
                    layout.name,
                    schema.name
                ));
            }
        }
    }

    for projection in &schema.projections {
        if projection.rebuildable && projection.rebuild_source.is_empty() {
            return Err(anyhow!(
                "Projection '{}' for table '{}' must declare a rebuild source",
                projection.name,
                schema.name
            ));
        }
    }

    Ok(())
}

/// Apply schema evolution changes and return a new schema.
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
                if new_schema.columns.iter().any(|c| &c.name == name) {
                    return Err(anyhow!("Column '{}' already exists", name));
                }
                let mut col = CatalogColumn::new(next_id, name, *data_type);
                next_id += 1;
                col.nullable = *nullable;
                col.default_value = default_value.clone();
                col.comment = comment.clone();
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
                let pos = new_schema
                    .columns
                    .iter()
                    .position(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;
                if new_schema.primary_key.contains(name) {
                    return Err(anyhow!("Cannot drop primary key column '{}'", name));
                }
                new_schema.columns.remove(pos);
                for idx in &mut new_schema.indexes {
                    idx.columns.retain(|c| c != name);
                }
                new_schema.indexes.retain(|idx| !idx.columns.is_empty());
            }
            SchemaChange::RenameColumn { old_name, new_name } => {
                let col = new_schema
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == old_name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", old_name))?;
                col.name = new_name.clone();
                for pk in &mut new_schema.primary_key {
                    if pk == old_name {
                        *pk = new_name.clone();
                    }
                }
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
            SchemaChange::MakeNotNullable { name } => {
                let col = new_schema
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;
                col.nullable = false;
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
            SchemaChange::MoveColumn { name, after } => {
                let pos = new_schema
                    .columns
                    .iter()
                    .position(|c| &c.name == name)
                    .ok_or_else(|| anyhow!("Column '{}' not found", name))?;
                let col = new_schema.columns.remove(pos);
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
                    new_schema.columns.insert(0, col);
                }
            }
            SchemaChange::AddConstraint {
                constraint_name,
                constraint,
            } => {
                let constraint_key = match &constraint {
                    ColumnConstraint::Unique { columns } => {
                        format!("constraint:unique:{}", columns.join(","))
                    }
                    ColumnConstraint::Check { .. } => {
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

                if let ColumnConstraint::Unique { columns } = &constraint {
                    for col_name in columns {
                        if !new_schema.columns.iter().any(|c| &c.name == col_name) {
                            return Err(anyhow!(
                                "Column '{}' not found for UNIQUE constraint",
                                col_name
                            ));
                        }
                    }
                    if let Some(name) = constraint_name {
                        let unique_index =
                            CatalogIndex::new(name, columns.clone(), CatalogIndexType::BTree)
                                .unique();
                        new_schema.indexes.push(unique_index);
                    }
                }
            }
            SchemaChange::DropConstraint { constraint_name } => {
                let keys_to_remove: Vec<String> = new_schema
                    .properties
                    .keys()
                    .filter(|k| {
                        k.starts_with("constraint:") && k.contains(constraint_name.as_str())
                    })
                    .cloned()
                    .collect();

                if keys_to_remove.is_empty() {
                    let idx_pos = new_schema
                        .indexes
                        .iter()
                        .position(|idx| &idx.name == constraint_name);
                    if let Some(pos) = idx_pos {
                        new_schema.indexes.remove(pos);
                    } else {
                        return Err(anyhow!("Constraint '{}' not found", constraint_name));
                    }
                } else {
                    for key in keys_to_remove {
                        new_schema.properties.remove(&key);
                    }
                    new_schema
                        .indexes
                        .retain(|idx| &idx.name != constraint_name);
                }
            }
            SchemaChange::PromotePropsKey {
                key,
                column_type,
                comment,
            } => {
                // Promoted column name: `props__<key>` (double underscore).
                let col_name = format!("props__{}", key);
                if new_schema.columns.iter().any(|c| c.name == col_name) {
                    return Err(anyhow!(
                        "Props key '{}' is already promoted to column '{}'",
                        key,
                        col_name
                    ));
                }
                // Promoted columns start at ID 100 to distinguish them from
                // canonical system columns (ID 1–9) and user columns (ID 10+).
                let promoted_id = new_schema
                    .columns
                    .iter()
                    .filter(|c| c.id >= 100)
                    .map(|c| c.id)
                    .max()
                    .unwrap_or(99)
                    + 1;
                let mut col = CatalogColumn::new(promoted_id, &col_name, *column_type);
                col.nullable = true;
                col.comment = comment.clone();
                col.properties
                    .insert("promoted_from_props".to_string(), key.clone());
                new_schema.columns.push(col);

                // Record the promotion so the compaction writer knows which
                // msgpack keys to route into the new typed column.
                new_schema
                    .props_auto_promotion
                    .promoted_keys
                    .insert(key.clone(), col_name);
            }
            SchemaChange::SetTableOption { key, value } => {
                match key.to_lowercase().as_str() {
                    "props_auto_promotion" => {
                        new_schema.props_auto_promotion.enabled =
                            matches!(value.to_lowercase().as_str(), "enabled" | "true" | "1");
                    }
                    _ => {
                        // Unknown options are stored as table properties so
                        // they round-trip without data loss.
                        new_schema.properties.insert(key.clone(), value.clone());
                    }
                }
            }
        }
    }

    validate_schema(&new_schema)?;
    Ok(new_schema)
}

/// Returns true when widening `from` → `to` is lossless.
pub fn is_compatible_type_change(from: CatalogDataType, to: CatalogDataType) -> bool {
    if from == to {
        return true;
    }
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

/// Returns the SQL type name for a catalog data type.
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
    use crate::{
        CatalogColumn, CatalogDataType, CatalogPhysicalFormat, CatalogProjection,
        CatalogStorageLayout, CatalogTableSchema,
    };

    fn base_schema() -> CatalogTableSchema {
        CatalogTableSchema::new("t")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int64).nullable(false))
            .with_column(CatalogColumn::new(2, "name", CatalogDataType::String))
    }

    #[test]
    fn validate_ok() {
        let schema = base_schema();
        assert!(validate_schema(&schema).is_ok());
    }

    #[test]
    fn rejects_reserved_system_column_names() {
        let schema = base_schema().with_column(CatalogColumn::new(
            3,
            "__proxima_deleted",
            CatalogDataType::Boolean,
        ));
        let err = validate_schema(&schema).expect_err("reserved column should fail");
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn validates_external_layout_contract() {
        let schema = base_schema().with_storage_layout(CatalogStorageLayout::federated_read(
            "raw",
            CatalogPhysicalFormat::Parquet,
            "s3://bucket/raw/",
        ));
        assert!(validate_schema(&schema).is_ok());
    }

    #[test]
    fn validate_empty_name_fails() {
        let mut schema = base_schema();
        schema.name = String::new();
        assert!(validate_schema(&schema).is_err());
    }

    #[test]
    fn add_column_roundtrip() {
        let schema = base_schema();
        let evolution = CatalogSchemaEvolution {
            changes: vec![SchemaChange::AddColumn {
                name: "age".to_string(),
                data_type: CatalogDataType::Int32,
                nullable: true,
                default_value: None,
                comment: None,
                after: None,
            }],
        };
        let new = apply_evolution(&schema, &evolution).unwrap();
        assert_eq!(new.columns.len(), 3);
        assert_eq!(new.schema_version, schema.schema_version + 1);
    }

    #[test]
    fn type_widening_compatible() {
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
    fn validate_schema_rejects_structural_storage_and_projection_contract_violations() {
        let mut empty_columns = CatalogTableSchema::new("t");
        empty_columns.columns.clear();
        assert!(
            validate_schema(&empty_columns)
                .unwrap_err()
                .to_string()
                .contains("at least one column")
        );

        let empty_column_name = CatalogTableSchema::new("t").with_column(CatalogColumn::new(
            1,
            "",
            CatalogDataType::Int64,
        ));
        assert!(
            validate_schema(&empty_column_name)
                .unwrap_err()
                .to_string()
                .contains("Column name cannot be empty")
        );

        let duplicate =
            base_schema().with_column(CatalogColumn::new(3, "id", CatalogDataType::Int64));
        assert!(
            validate_schema(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("Duplicate column")
        );

        let missing_pk = base_schema().with_primary_key(vec!["missing".to_string()]);
        assert!(
            validate_schema(&missing_pk)
                .unwrap_err()
                .to_string()
                .contains("Primary key column")
        );

        let missing_index_col = base_schema().with_index(CatalogIndex::new(
            "bad_idx",
            vec!["missing".to_string()],
            CatalogIndexType::BTree,
        ));
        assert!(
            validate_schema(&missing_index_col)
                .unwrap_err()
                .to_string()
                .contains("references non-existent column")
        );

        let vector_missing_dimension =
            base_schema().with_column(CatalogColumn::new(3, "embedding", CatalogDataType::Vector));
        assert!(
            validate_schema(&vector_missing_dimension)
                .unwrap_err()
                .to_string()
                .contains("dimension")
        );

        let mut vector_col = CatalogColumn::new(3, "embedding", CatalogDataType::Vector);
        vector_col
            .properties
            .insert("dimension".to_string(), "384".to_string());
        assert!(validate_schema(&base_schema().with_column(vector_col)).is_ok());

        let mut external = CatalogStorageLayout::federated_read(
            "raw",
            CatalogPhysicalFormat::Parquet,
            "s3://bucket/raw",
        );
        external.location = None;
        assert!(
            validate_schema(&base_schema().with_storage_layout(external))
                .unwrap_err()
                .to_string()
                .contains("must declare a location")
        );

        let mut external = CatalogStorageLayout::federated_read(
            "raw",
            CatalogPhysicalFormat::Parquet,
            "s3://bucket/raw",
        );
        external.snapshot_semantics = None;
        assert!(
            validate_schema(&base_schema().with_storage_layout(external))
                .unwrap_err()
                .to_string()
                .contains("snapshot semantics")
        );

        let mut projection = CatalogProjection::rebuildable(
            "ann",
            crate::CatalogProjectionKind::VectorAnn,
            "primary",
        );
        projection.rebuild_source.clear();
        assert!(
            validate_schema(&base_schema().with_projection(projection))
                .unwrap_err()
                .to_string()
                .contains("rebuild source")
        );
    }

    #[test]
    fn apply_evolution_covers_column_mutation_and_error_paths() {
        let schema = base_schema()
            .with_primary_key(vec!["id".to_string()])
            .with_index(CatalogIndex::new(
                "name_idx",
                vec!["name".to_string()],
                CatalogIndexType::BTree,
            ));

        let evolved = apply_evolution(
            &schema,
            &CatalogSchemaEvolution {
                changes: vec![
                    SchemaChange::AddColumn {
                        name: "age".to_string(),
                        data_type: CatalogDataType::Int32,
                        nullable: true,
                        default_value: Some("0".to_string()),
                        comment: Some("Age".to_string()),
                        after: Some("id".to_string()),
                    },
                    SchemaChange::UpdateComment {
                        name: "age".to_string(),
                        comment: "Years".to_string(),
                    },
                    SchemaChange::MakeNotNullable {
                        name: "age".to_string(),
                    },
                    SchemaChange::SetDefault {
                        name: "age".to_string(),
                        default_value: "18".to_string(),
                    },
                    SchemaChange::MakeNullable {
                        name: "age".to_string(),
                    },
                    SchemaChange::DropDefault {
                        name: "age".to_string(),
                    },
                    SchemaChange::MoveColumn {
                        name: "age".to_string(),
                        after: None,
                    },
                    SchemaChange::RenameColumn {
                        old_name: "name".to_string(),
                        new_name: "display_name".to_string(),
                    },
                    SchemaChange::ChangeType {
                        name: "age".to_string(),
                        new_type: CatalogDataType::Int64,
                    },
                ],
            },
        )
        .unwrap();

        assert_eq!(evolved.columns[0].name, "age");
        let age = evolved
            .columns
            .iter()
            .find(|col| col.name == "age")
            .unwrap();
        assert_eq!(age.data_type, CatalogDataType::Int64);
        assert!(age.nullable);
        assert_eq!(age.default_value, None);
        assert_eq!(age.comment.as_deref(), Some("Years"));
        assert_eq!(evolved.indexes[0].columns, vec!["display_name"]);

        let dropped = apply_evolution(
            &evolved,
            &CatalogSchemaEvolution {
                changes: vec![SchemaChange::DropColumn {
                    name: "display_name".to_string(),
                }],
            },
        )
        .unwrap();
        assert!(!dropped.columns.iter().any(|col| col.name == "display_name"));
        assert!(dropped.indexes.is_empty());

        for change in [
            SchemaChange::AddColumn {
                name: "id".to_string(),
                data_type: CatalogDataType::Int64,
                nullable: false,
                default_value: None,
                comment: None,
                after: None,
            },
            SchemaChange::DropColumn {
                name: "id".to_string(),
            },
            SchemaChange::RenameColumn {
                old_name: "missing".to_string(),
                new_name: "x".to_string(),
            },
            SchemaChange::ChangeType {
                name: "name".to_string(),
                new_type: CatalogDataType::Binary,
            },
            SchemaChange::MoveColumn {
                name: "name".to_string(),
                after: Some("missing".to_string()),
            },
        ] {
            assert!(
                apply_evolution(
                    &schema,
                    &CatalogSchemaEvolution {
                        changes: vec![change]
                    }
                )
                .is_err()
            );
        }
    }

    #[test]
    fn apply_evolution_covers_constraints_props_promotion_and_table_options() {
        let schema = base_schema();
        let evolved = apply_evolution(
            &schema,
            &CatalogSchemaEvolution {
                changes: vec![
                    SchemaChange::AddConstraint {
                        constraint_name: Some("uniq_name".to_string()),
                        constraint: ColumnConstraint::Unique {
                            columns: vec!["name".to_string()],
                        },
                    },
                    SchemaChange::AddConstraint {
                        constraint_name: Some("check_name".to_string()),
                        constraint: ColumnConstraint::Check {
                            expression: "name <> ''".to_string(),
                        },
                    },
                    SchemaChange::AddConstraint {
                        constraint_name: Some("fk_name".to_string()),
                        constraint: ColumnConstraint::ForeignKey {
                            columns: vec!["name".to_string()],
                            references_table: "other".to_string(),
                            references_columns: vec!["name".to_string()],
                            on_delete: None,
                            on_update: None,
                        },
                    },
                    SchemaChange::PromotePropsKey {
                        key: "status".to_string(),
                        column_type: CatalogDataType::String,
                        comment: Some("Promoted status".to_string()),
                    },
                    SchemaChange::SetTableOption {
                        key: "props_auto_promotion".to_string(),
                        value: "enabled".to_string(),
                    },
                    SchemaChange::SetTableOption {
                        key: "custom_option".to_string(),
                        value: "custom_value".to_string(),
                    },
                ],
            },
        )
        .unwrap();

        assert!(
            evolved
                .indexes
                .iter()
                .any(|idx| idx.name == "uniq_name" && idx.is_unique)
        );
        assert!(
            evolved
                .properties
                .keys()
                .any(|key| key.starts_with("constraint:check"))
        );
        assert!(
            evolved
                .properties
                .keys()
                .any(|key| key.starts_with("constraint:fk"))
        );
        assert_eq!(
            evolved
                .props_auto_promotion
                .promoted_keys
                .get("status")
                .map(String::as_str),
            Some("props__status")
        );
        assert!(evolved.props_auto_promotion.enabled);
        assert_eq!(
            evolved.properties.get("custom_option").map(String::as_str),
            Some("custom_value")
        );

        let dropped = apply_evolution(
            &evolved,
            &CatalogSchemaEvolution {
                changes: vec![SchemaChange::DropConstraint {
                    constraint_name: "uniq_name".to_string(),
                }],
            },
        )
        .unwrap();
        assert!(!dropped.indexes.iter().any(|idx| idx.name == "uniq_name"));

        for change in [
            SchemaChange::AddConstraint {
                constraint_name: Some("bad_unique".to_string()),
                constraint: ColumnConstraint::Unique {
                    columns: vec!["missing".to_string()],
                },
            },
            SchemaChange::DropConstraint {
                constraint_name: "missing".to_string(),
            },
            SchemaChange::PromotePropsKey {
                key: "status".to_string(),
                column_type: CatalogDataType::String,
                comment: None,
            },
        ] {
            assert!(
                apply_evolution(
                    &evolved,
                    &CatalogSchemaEvolution {
                        changes: vec![change]
                    }
                )
                .is_err()
            );
        }
    }

    #[test]
    fn sql_type_names_cover_every_catalog_type() {
        let names = [
            (CatalogDataType::Boolean, "BOOLEAN"),
            (CatalogDataType::Int8, "TINYINT"),
            (CatalogDataType::Int16, "SMALLINT"),
            (CatalogDataType::Int32, "INTEGER"),
            (CatalogDataType::Int64, "BIGINT"),
            (CatalogDataType::Float32, "REAL"),
            (CatalogDataType::Float64, "DOUBLE PRECISION"),
            (CatalogDataType::String, "TEXT"),
            (CatalogDataType::Binary, "BYTEA"),
            (CatalogDataType::Date, "DATE"),
            (CatalogDataType::Timestamp, "TIMESTAMP"),
            (CatalogDataType::TimestampTz, "TIMESTAMP WITH TIME ZONE"),
            (CatalogDataType::Time, "TIME"),
            (CatalogDataType::Uuid, "UUID"),
            (CatalogDataType::Json, "JSONB"),
            (CatalogDataType::Decimal, "DECIMAL"),
            (CatalogDataType::Vector, "VECTOR"),
            (CatalogDataType::SparseVector, "SPARSE_VECTOR"),
            (CatalogDataType::BinaryVector, "BINARY_VECTOR"),
        ];

        for (data_type, expected) in names {
            assert_eq!(sql_type_name(data_type), expected);
        }

        assert!(is_compatible_type_change(
            CatalogDataType::Int8,
            CatalogDataType::Int16
        ));
        assert!(is_compatible_type_change(
            CatalogDataType::Int8,
            CatalogDataType::Int32
        ));
        assert!(is_compatible_type_change(
            CatalogDataType::Int8,
            CatalogDataType::Int64
        ));
        assert!(is_compatible_type_change(
            CatalogDataType::Int16,
            CatalogDataType::Int32
        ));
        assert!(is_compatible_type_change(
            CatalogDataType::Int16,
            CatalogDataType::Int64
        ));
        assert!(is_compatible_type_change(
            CatalogDataType::Int64,
            CatalogDataType::Float64
        ));
    }
}
