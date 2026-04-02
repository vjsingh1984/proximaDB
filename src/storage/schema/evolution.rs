//! # Schema Evolution - ADD/DROP/RENAME Column Operations
//!
//! Provides schema evolution operations compatible with Iceberg/Delta Lake patterns.
//! Supports backward and forward compatibility checks for compute engine integration.

use anyhow::Result;
use async_trait::async_trait;

use super::proxima_schema::{DefaultValue, ProximaColumn, ProximaDataType, ProximaSchema};

/// Schema evolution operations.
#[derive(Debug, Clone)]
pub enum SchemaEvolutionOp {
    /// Add a new column
    AddColumn {
        column: ProximaColumn,
        /// Position: None = end, Some(id) = after column with this ID
        after_column_id: Option<i32>,
    },

    /// Drop a column (soft delete with tombstone)
    DropColumn { column_id: i32 },

    /// Rename a column (preserves column ID)
    RenameColumn { column_id: i32, new_name: String },

    /// Change column type (must be compatible)
    ChangeType {
        column_id: i32,
        new_type: ProximaDataType,
        /// Optional conversion expression
        conversion: Option<String>,
    },

    /// Make column nullable
    MakeNullable { column_id: i32 },

    /// Make column NOT NULL (requires default or existing data check)
    MakeNotNullable {
        column_id: i32,
        default_for_nulls: DefaultValue,
    },

    /// Set default value
    SetDefault {
        column_id: i32,
        default: DefaultValue,
    },

    /// Remove default value
    DropDefault { column_id: i32 },

    /// Reorder columns
    ReorderColumns {
        /// New column order by ID
        column_order: Vec<i32>,
    },

    /// Update column comment
    SetComment { column_id: i32, comment: String },
}

/// Result of schema evolution validation.
#[derive(Debug)]
pub struct EvolutionValidation {
    /// Whether the evolution is valid
    pub is_valid: bool,
    /// Whether backward compatible
    pub is_backward_compatible: bool,
    /// Whether forward compatible
    pub is_forward_compatible: bool,
    /// Validation warnings
    pub warnings: Vec<String>,
    /// Validation errors
    pub errors: Vec<String>,
    /// Whether data migration is required
    pub data_migration_required: bool,
    /// Estimated migration cost
    pub estimated_migration_cost: MigrationCost,
}

impl Default for EvolutionValidation {
    fn default() -> Self {
        Self {
            is_valid: true,
            is_backward_compatible: true,
            is_forward_compatible: true,
            warnings: Vec::new(),
            errors: Vec::new(),
            data_migration_required: false,
            estimated_migration_cost: MigrationCost::default(),
        }
    }
}

/// Estimated cost of data migration.
#[derive(Debug, Default)]
pub struct MigrationCost {
    /// Estimated rows to process
    pub rows_affected: u64,
    /// Estimated bytes to rewrite
    pub bytes_to_rewrite: u64,
    /// Whether migration can be done online
    pub online_capable: bool,
}

/// Type compatibility rules for schema evolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCompatibility {
    /// Types are identical
    Identical,
    /// Safe widening (e.g., Int32 -> Int64)
    SafeWidening,
    /// Lossy narrowing (e.g., Int64 -> Int32)
    LossyNarrowing,
    /// String coercion possible
    StringCoercion,
    /// Incompatible types
    Incompatible,
}

/// Migration plan for schema evolution.
#[derive(Debug)]
pub struct MigrationPlan {
    /// Source schema version
    pub from_version: u32,
    /// Target schema version
    pub to_version: u32,
    /// Steps to execute
    pub steps: Vec<MigrationStep>,
    /// Can be executed online (without downtime)
    pub is_online: bool,
    /// Estimated duration in seconds
    pub estimated_duration_secs: u64,
}

/// Individual migration step.
#[derive(Debug)]
pub enum MigrationStep {
    /// Add column with default value (no data migration needed)
    AddColumnWithDefault {
        /// Column ID
        column_id: i32,
        /// Default value
        default: DefaultValue,
    },
    /// Rewrite data files with new schema
    RewriteDataFiles {
        /// Affected files
        affected_files: Vec<String>,
        /// Transformations to apply
        transformations: Vec<ColumnTransformation>,
    },
    /// Update metadata only (rename, comment, etc.)
    UpdateMetadataOnly {
        /// Metadata changes
        changes: Vec<MetadataChange>,
    },
    /// Create new index for added column
    CreateIndex {
        /// Column ID
        column_id: i32,
        /// Index type
        index_type: String,
    },
}

/// Column transformation during migration.
#[derive(Debug)]
pub struct ColumnTransformation {
    /// Source column ID
    pub source_column_id: i32,
    /// Target column ID
    pub target_column_id: i32,
    /// SQL transformation expression
    pub transformation: String,
}

/// Metadata change during migration.
#[derive(Debug)]
pub struct MetadataChange {
    /// Column ID
    pub column_id: i32,
    /// Type of change
    pub change_type: String,
    /// Old value
    pub old_value: String,
    /// New value
    pub new_value: String,
}

/// Schema evolution trait for storage engines.
#[async_trait]
pub trait SchemaEvolution: Send + Sync {
    /// Validate evolution operations against current schema.
    async fn validate_evolution(
        &self,
        current_schema: &ProximaSchema,
        operations: &[SchemaEvolutionOp],
    ) -> Result<EvolutionValidation>;

    /// Apply evolution operations to create new schema version.
    async fn evolve_schema(
        &self,
        current_schema: &ProximaSchema,
        operations: &[SchemaEvolutionOp],
    ) -> Result<ProximaSchema>;

    /// Check type compatibility for type change operations.
    fn check_type_compatibility(
        &self,
        old_type: &ProximaDataType,
        new_type: &ProximaDataType,
    ) -> TypeCompatibility;

    /// Get migration strategy for schema change.
    async fn plan_migration(
        &self,
        from_schema: &ProximaSchema,
        to_schema: &ProximaSchema,
    ) -> Result<MigrationPlan>;
}

/// Default implementation of schema evolution.
pub struct DefaultSchemaEvolution {
    /// Compatible type widening rules
    widening_rules: Vec<(ProximaDataType, ProximaDataType)>,
}

impl Default for DefaultSchemaEvolution {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultSchemaEvolution {
    pub fn new() -> Self {
        Self {
            widening_rules: vec![
                // Integer widening
                (ProximaDataType::Int8, ProximaDataType::Int16),
                (ProximaDataType::Int8, ProximaDataType::Int32),
                (ProximaDataType::Int8, ProximaDataType::Int64),
                (ProximaDataType::Int16, ProximaDataType::Int32),
                (ProximaDataType::Int16, ProximaDataType::Int64),
                (ProximaDataType::Int32, ProximaDataType::Int64),
                // Unsigned integer widening
                (ProximaDataType::UInt8, ProximaDataType::UInt16),
                (ProximaDataType::UInt8, ProximaDataType::UInt32),
                (ProximaDataType::UInt8, ProximaDataType::UInt64),
                (ProximaDataType::UInt16, ProximaDataType::UInt32),
                (ProximaDataType::UInt16, ProximaDataType::UInt64),
                (ProximaDataType::UInt32, ProximaDataType::UInt64),
                // Float widening
                (ProximaDataType::Float32, ProximaDataType::Float64),
            ],
        }
    }

    fn is_safe_widening(&self, old: &ProximaDataType, new: &ProximaDataType) -> bool {
        self.widening_rules
            .iter()
            .any(|(from, to)| from == old && to == new)
    }
}

#[async_trait]
impl SchemaEvolution for DefaultSchemaEvolution {
    async fn validate_evolution(
        &self,
        current_schema: &ProximaSchema,
        operations: &[SchemaEvolutionOp],
    ) -> Result<EvolutionValidation> {
        let mut validation = EvolutionValidation::default();

        for op in operations {
            match op {
                SchemaEvolutionOp::AddColumn { column, .. } => {
                    // Adding nullable column is backward compatible
                    if !column.nullable && column.default_value.is_none() {
                        validation.errors.push(format!(
                            "Cannot add non-nullable column '{}' without default value",
                            column.name
                        ));
                        validation.is_valid = false;
                    }
                    // Adding column breaks forward compatibility for old readers
                    validation.is_forward_compatible = false;
                }
                SchemaEvolutionOp::DropColumn { column_id } => {
                    // Dropping column breaks backward compatibility
                    validation.is_backward_compatible = false;
                    if current_schema.primary_key.contains(column_id) {
                        validation
                            .errors
                            .push(format!("Cannot drop primary key column {}", column_id));
                        validation.is_valid = false;
                    }
                }
                SchemaEvolutionOp::ChangeType {
                    column_id,
                    new_type,
                    ..
                } => {
                    if let Some(col) = current_schema.column_by_id(*column_id) {
                        let compat = self.check_type_compatibility(&col.data_type, new_type);
                        match compat {
                            TypeCompatibility::Incompatible => {
                                validation.errors.push(format!(
                                    "Incompatible type change for column '{}': {:?} -> {:?}",
                                    col.name, col.data_type, new_type
                                ));
                                validation.is_valid = false;
                            }
                            TypeCompatibility::LossyNarrowing => {
                                validation.warnings.push(format!(
                                    "Lossy type change for column '{}': {:?} -> {:?}",
                                    col.name, col.data_type, new_type
                                ));
                                validation.is_backward_compatible = false;
                                validation.data_migration_required = true;
                            }
                            TypeCompatibility::SafeWidening => {
                                validation.data_migration_required = true;
                            }
                            _ => {}
                        }
                    } else {
                        validation
                            .errors
                            .push(format!("Column {} not found", column_id));
                        validation.is_valid = false;
                    }
                }
                SchemaEvolutionOp::MakeNotNullable { column_id, .. } => {
                    validation.warnings.push(format!(
                        "Making column {} NOT NULL requires checking existing data",
                        column_id
                    ));
                    validation.data_migration_required = true;
                }
                _ => {}
            }
        }

        Ok(validation)
    }

    async fn evolve_schema(
        &self,
        current_schema: &ProximaSchema,
        operations: &[SchemaEvolutionOp],
    ) -> Result<ProximaSchema> {
        let mut new_schema = current_schema.clone();
        new_schema.version += 1;
        new_schema.parent_schema_id = Some(current_schema.schema_id.clone());
        new_schema.schema_id = uuid::Uuid::new_v4().to_string();
        new_schema.created_at_ms = chrono::Utc::now().timestamp_millis();
        new_schema.is_legacy_vector_record = false;

        for op in operations {
            match op {
                SchemaEvolutionOp::AddColumn {
                    column,
                    after_column_id,
                } => {
                    let mut col = column.clone();
                    col.id = new_schema.next_column_id();

                    if let Some(after_id) = after_column_id {
                        let pos = new_schema
                            .columns
                            .iter()
                            .position(|c| c.id == *after_id)
                            .map_or(new_schema.columns.len(), |p| p + 1);
                        new_schema.columns.insert(pos, col);
                    } else {
                        new_schema.columns.push(col);
                    }
                }
                SchemaEvolutionOp::DropColumn { column_id } => {
                    if let Some(col) = new_schema.columns.iter_mut().find(|c| c.id == *column_id) {
                        col.is_deleted = true;
                    }
                }
                SchemaEvolutionOp::RenameColumn {
                    column_id,
                    new_name,
                } => {
                    if let Some(col) = new_schema.columns.iter_mut().find(|c| c.id == *column_id) {
                        if col.original_id.is_none() {
                            col.original_id = Some(col.id);
                        }
                        col.name = new_name.clone();
                    }
                }
                SchemaEvolutionOp::ChangeType {
                    column_id,
                    new_type,
                    ..
                } => {
                    if let Some(col) = new_schema.columns.iter_mut().find(|c| c.id == *column_id) {
                        col.data_type = new_type.clone();
                    }
                }
                SchemaEvolutionOp::MakeNullable { column_id } => {
                    if let Some(col) = new_schema.columns.iter_mut().find(|c| c.id == *column_id) {
                        col.nullable = true;
                    }
                }
                SchemaEvolutionOp::MakeNotNullable { column_id, .. } => {
                    if let Some(col) = new_schema.columns.iter_mut().find(|c| c.id == *column_id) {
                        col.nullable = false;
                    }
                }
                SchemaEvolutionOp::SetDefault { column_id, default } => {
                    if let Some(col) = new_schema.columns.iter_mut().find(|c| c.id == *column_id) {
                        col.default_value = Some(default.clone());
                    }
                }
                SchemaEvolutionOp::DropDefault { column_id } => {
                    if let Some(col) = new_schema.columns.iter_mut().find(|c| c.id == *column_id) {
                        col.default_value = None;
                    }
                }
                SchemaEvolutionOp::ReorderColumns { column_order } => {
                    let mut reordered = Vec::new();
                    for id in column_order {
                        if let Some(col) = new_schema.columns.iter().find(|c| c.id == *id) {
                            reordered.push(col.clone());
                        }
                    }
                    // Add any columns not in the order list at the end
                    for col in &new_schema.columns {
                        if !column_order.contains(&col.id) {
                            reordered.push(col.clone());
                        }
                    }
                    new_schema.columns = reordered;
                }
                SchemaEvolutionOp::SetComment { column_id, comment } => {
                    if let Some(col) = new_schema.columns.iter_mut().find(|c| c.id == *column_id) {
                        col.comment = Some(comment.clone());
                    }
                }
            }
        }

        // Recompute fingerprint
        new_schema.fingerprint =
            ProximaSchema::compute_fingerprint_for_columns(&new_schema.columns);

        Ok(new_schema)
    }

    fn check_type_compatibility(
        &self,
        old_type: &ProximaDataType,
        new_type: &ProximaDataType,
    ) -> TypeCompatibility {
        if old_type == new_type {
            return TypeCompatibility::Identical;
        }

        if self.is_safe_widening(old_type, new_type) {
            return TypeCompatibility::SafeWidening;
        }

        if self.is_safe_widening(new_type, old_type) {
            return TypeCompatibility::LossyNarrowing;
        }

        // Check string coercion
        match (old_type, new_type) {
            (_, ProximaDataType::String) => TypeCompatibility::StringCoercion,
            (ProximaDataType::String, _) => TypeCompatibility::LossyNarrowing,
            _ => TypeCompatibility::Incompatible,
        }
    }

    async fn plan_migration(
        &self,
        from_schema: &ProximaSchema,
        to_schema: &ProximaSchema,
    ) -> Result<MigrationPlan> {
        let mut steps = Vec::new();
        let mut is_online = true;

        // Check for added columns
        for col in &to_schema.columns {
            if !col.is_deleted && from_schema.column_by_id(col.id).is_none() {
                if let Some(ref default) = col.default_value {
                    steps.push(MigrationStep::AddColumnWithDefault {
                        column_id: col.id,
                        default: default.clone(),
                    });
                } else {
                    is_online = false;
                }
            }
        }

        // Check for type changes
        for col in &to_schema.columns {
            if !col.is_deleted
                && let Some(old_col) = from_schema.column_by_id(col.id)
                    && old_col.data_type != col.data_type {
                        is_online = false;
                        steps.push(MigrationStep::RewriteDataFiles {
                            affected_files: vec![], // To be filled by storage engine
                            transformations: vec![ColumnTransformation {
                                source_column_id: old_col.id,
                                target_column_id: col.id,
                                transformation: format!(
                                    "CAST({} AS {:?})",
                                    old_col.name, col.data_type
                                ),
                            }],
                        });
                    }
        }

        Ok(MigrationPlan {
            from_version: from_schema.version,
            to_version: to_schema.version,
            steps,
            is_online,
            estimated_duration_secs: 0, // To be estimated by storage engine
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_add_nullable_column() {
        let evolution = DefaultSchemaEvolution::new();
        let schema = ProximaSchema::vector_record_schema(512);

        let ops = vec![SchemaEvolutionOp::AddColumn {
            column: ProximaColumn {
                id: 0, // Will be assigned
                name: "category".to_string(),
                data_type: ProximaDataType::String,
                nullable: true,
                default_value: None,
                comment: None,
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            after_column_id: None,
        }];

        let validation = evolution.validate_evolution(&schema, &ops).await.unwrap();
        assert!(validation.is_valid);
        assert!(validation.is_backward_compatible);

        let new_schema = evolution.evolve_schema(&schema, &ops).await.unwrap();
        assert_eq!(new_schema.version, 1);
        assert_eq!(new_schema.active_column_count(), 6);
        assert!(new_schema.column_by_name("category").is_some());
    }

    #[tokio::test]
    async fn test_add_non_nullable_without_default_fails() {
        let evolution = DefaultSchemaEvolution::new();
        let schema = ProximaSchema::vector_record_schema(512);

        let ops = vec![SchemaEvolutionOp::AddColumn {
            column: ProximaColumn {
                id: 0,
                name: "required_field".to_string(),
                data_type: ProximaDataType::String,
                nullable: false,
                default_value: None, // No default!
                comment: None,
                metadata: HashMap::new(),
                is_deleted: false,
                original_id: None,
            },
            after_column_id: None,
        }];

        let validation = evolution.validate_evolution(&schema, &ops).await.unwrap();
        assert!(!validation.is_valid);
        assert!(!validation.errors.is_empty());
    }

    #[tokio::test]
    async fn test_rename_column() {
        let evolution = DefaultSchemaEvolution::new();
        let schema = ProximaSchema::vector_record_schema(512);

        let ops = vec![SchemaEvolutionOp::RenameColumn {
            column_id: 3, // metadata column
            new_name: "properties".to_string(),
        }];

        let new_schema = evolution.evolve_schema(&schema, &ops).await.unwrap();
        assert!(new_schema.column_by_name("properties").is_some());
        assert!(new_schema.column_by_name("metadata").is_none());
    }

    #[test]
    fn test_type_compatibility() {
        let evolution = DefaultSchemaEvolution::new();

        assert_eq!(
            evolution.check_type_compatibility(&ProximaDataType::Int32, &ProximaDataType::Int64),
            TypeCompatibility::SafeWidening
        );

        assert_eq!(
            evolution.check_type_compatibility(&ProximaDataType::Int64, &ProximaDataType::Int32),
            TypeCompatibility::LossyNarrowing
        );

        assert_eq!(
            evolution.check_type_compatibility(&ProximaDataType::Int32, &ProximaDataType::String),
            TypeCompatibility::StringCoercion
        );
    }
}
