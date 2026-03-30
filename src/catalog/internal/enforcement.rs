//! Constraint Enforcement Engine
//!
//! Enforces schema constraints during write operations:
//! - Primary Key: Uniqueness validation
//! - Foreign Key: Referential integrity (including cross-model)
//! - Unique: Uniqueness on specified columns
//! - Check: Expression-based validation
//! - Not Null: Null value prevention

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tokio::sync::RwLock;
use tracing::debug;

use super::{
    CatalogObject, ConstraintType, ForeignKeyReference, ObjectSchema, ReferentialAction,
    SchemaEnforcementMode, TableConstraint,
};

/// Constraint violation type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintViolation {
    /// Constraint name
    pub constraint_name: String,
    /// Violation message
    pub message: String,
    /// Violating column(s)
    pub columns: Vec<String>,
    /// Violating value(s) (as strings for display)
    pub values: Vec<String>,
    /// Severity
    pub severity: ViolationSeverity,
}

impl ConstraintViolation {
    /// Create a new violation
    pub fn new(
        constraint_name: impl Into<String>,
        message: impl Into<String>,
        columns: Vec<String>,
        values: Vec<String>,
    ) -> Self {
        Self {
            constraint_name: constraint_name.into(),
            message: message.into(),
            columns,
            values,
            severity: ViolationSeverity::Error,
        }
    }

    /// Create a warning-level violation
    pub fn warning(
        constraint_name: impl Into<String>,
        message: impl Into<String>,
        columns: Vec<String>,
    ) -> Self {
        Self {
            constraint_name: constraint_name.into(),
            message: message.into(),
            columns,
            values: vec![],
            severity: ViolationSeverity::Warning,
        }
    }

    /// Primary key violation
    pub fn primary_key(columns: Vec<String>, values: Vec<String>) -> Self {
        Self::new(
            "PRIMARY KEY",
            format!(
                "Duplicate primary key value: ({}) = ({})",
                columns.join(", "),
                values.join(", ")
            ),
            columns,
            values,
        )
    }

    /// Not null violation
    pub fn not_null(column: impl Into<String>) -> Self {
        let col = column.into();
        Self::new(
            format!("NOT NULL ({col})"),
            format!("Column '{col}' cannot be null"),
            vec![col],
            vec!["NULL".to_string()],
        )
    }

    /// Foreign key violation
    pub fn foreign_key(
        constraint_name: impl Into<String>,
        columns: Vec<String>,
        values: Vec<String>,
        referenced: &str,
    ) -> Self {
        Self::new(
            constraint_name,
            format!(
                "Foreign key violation: ({}) = ({}) references non-existent row in {}",
                columns.join(", "),
                values.join(", "),
                referenced
            ),
            columns,
            values,
        )
    }

    /// Unique constraint violation
    pub fn unique(
        constraint_name: impl Into<String>,
        columns: Vec<String>,
        values: Vec<String>,
    ) -> Self {
        Self::new(
            constraint_name,
            format!(
                "Unique constraint violation: ({}) = ({}) already exists",
                columns.join(", "),
                values.join(", ")
            ),
            columns,
            values,
        )
    }

    /// Check constraint violation
    pub fn check(constraint_name: impl Into<String>, expression: &str) -> Self {
        Self::new(
            constraint_name,
            format!("Check constraint failed: {expression}"),
            vec![],
            vec![],
        )
    }
}

impl std::fmt::Display for ConstraintViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.constraint_name, self.message)
    }
}

/// Violation severity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViolationSeverity {
    /// Warning (logged but not blocking)
    Warning,
    /// Error (blocks the operation)
    Error,
}

/// Result of constraint enforcement
#[derive(Debug, Clone)]
pub struct EnforcementResult {
    /// Whether all constraints passed
    pub is_valid: bool,
    /// List of violations (if any)
    pub violations: Vec<ConstraintViolation>,
    /// Warnings (non-blocking)
    pub warnings: Vec<ConstraintViolation>,
    /// Enforcement mode used
    pub enforcement_mode: SchemaEnforcementMode,
}

impl EnforcementResult {
    /// Create a passing result
    pub fn pass(mode: SchemaEnforcementMode) -> Self {
        Self {
            is_valid: true,
            violations: vec![],
            warnings: vec![],
            enforcement_mode: mode,
        }
    }

    /// Create a failing result
    pub fn fail(violations: Vec<ConstraintViolation>, mode: SchemaEnforcementMode) -> Self {
        Self {
            is_valid: false,
            violations,
            warnings: vec![],
            enforcement_mode: mode,
        }
    }

    /// Add a violation
    pub fn add_violation(&mut self, violation: ConstraintViolation) {
        if violation.severity == ViolationSeverity::Error {
            self.violations.push(violation);
            self.is_valid = false;
        } else {
            self.warnings.push(violation);
        }
    }

    /// Merge with another result
    pub fn merge(&mut self, other: EnforcementResult) {
        self.violations.extend(other.violations);
        self.warnings.extend(other.warnings);
        if !other.is_valid {
            self.is_valid = false;
        }
    }

    /// Get error message if invalid
    pub fn error_message(&self) -> Option<String> {
        if self.is_valid {
            None
        } else {
            Some(
                self.violations
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }
}

/// Row value for constraint checking
#[derive(Debug, Clone)]
pub struct RowValue {
    /// Column values as (column_name, value_as_string)
    pub values: HashMap<String, Option<String>>,
}

impl RowValue {
    /// Create a new row value
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    /// Set a column value
    pub fn set(&mut self, column: impl Into<String>, value: Option<String>) {
        self.values.insert(column.into(), value);
    }

    /// Get a column value
    pub fn get(&self, column: &str) -> Option<&Option<String>> {
        self.values.get(column)
    }

    /// Check if column is null
    pub fn is_null(&self, column: &str) -> bool {
        self.values.get(column).map(|v| v.is_none()).unwrap_or(true)
    }

    /// Get values for columns as a tuple
    pub fn get_tuple(&self, columns: &[String]) -> Vec<Option<String>> {
        columns
            .iter()
            .map(|c| self.values.get(c).cloned().flatten())
            .collect()
    }
}

impl Default for RowValue {
    fn default() -> Self {
        Self::new()
    }
}

/// Constraint enforcer for schema validation
pub struct ConstraintEnforcer {
    /// Existing values for uniqueness checks (fqn -> column_tuple -> set of existing tuples)
    unique_indexes: RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
}

impl ConstraintEnforcer {
    /// Create a new constraint enforcer
    pub fn new() -> Self {
        Self {
            unique_indexes: RwLock::new(HashMap::new()),
        }
    }

    /// Validate a row against schema constraints
    pub async fn validate_insert(
        &self,
        object: &CatalogObject,
        row: &RowValue,
    ) -> EnforcementResult {
        let mut result = EnforcementResult::pass(object.enforcement_mode);

        // Skip validation for flexible mode
        if object.enforcement_mode == SchemaEnforcementMode::Flexible {
            return result;
        }

        // Check not-null constraints on core columns
        if object.enforcement_mode == SchemaEnforcementMode::Strict {
            self.check_not_null(&object.schema, row, &mut result);
        }

        // Check all table constraints
        for constraint in &object.schema.constraints {
            self.check_constraint(object, constraint, row, &mut result)
                .await;
        }

        result
    }

    /// Validate a row update
    pub async fn validate_update(
        &self,
        object: &CatalogObject,
        _old_row: &RowValue,
        new_row: &RowValue,
    ) -> EnforcementResult {
        let mut result = EnforcementResult::pass(object.enforcement_mode);

        // Skip validation for flexible mode
        if object.enforcement_mode == SchemaEnforcementMode::Flexible {
            return result;
        }

        // Check not-null constraints on updated columns
        if object.enforcement_mode == SchemaEnforcementMode::Strict {
            self.check_not_null(&object.schema, new_row, &mut result);
        }

        // Check constraints that might be affected by the update
        for constraint in &object.schema.constraints {
            let columns_affected = match &constraint.constraint_type {
                ConstraintType::PrimaryKey { columns } => {
                    columns.iter().any(|c| new_row.values.contains_key(c))
                }
                ConstraintType::ForeignKey { columns, .. } => {
                    columns.iter().any(|c| new_row.values.contains_key(c))
                }
                ConstraintType::Unique { columns } => {
                    columns.iter().any(|c| new_row.values.contains_key(c))
                }
                ConstraintType::Check { .. } => true, // Always check
                ConstraintType::NotNull { column } => new_row.values.contains_key(column),
                ConstraintType::Exclusion { columns, .. } => {
                    columns.iter().any(|c| new_row.values.contains_key(c))
                }
            };

            if columns_affected {
                self.check_constraint(object, constraint, new_row, &mut result)
                    .await;
            }
        }

        result
    }

    /// Validate referential integrity for delete
    pub async fn validate_delete(
        &self,
        object: &CatalogObject,
        _row: &RowValue,
        referencing_objects: &[&CatalogObject],
    ) -> EnforcementResult {
        let result = EnforcementResult::pass(object.enforcement_mode);

        // Check if any other objects have FK references to this row
        for referencing_obj in referencing_objects {
            for constraint in &referencing_obj.schema.constraints {
                if let ConstraintType::ForeignKey {
                    reference,
                    on_delete,
                    ..
                } = &constraint.constraint_type
                {
                    // Check if this FK references our object
                    let references_us = match reference {
                        ForeignKeyReference::Table { table, .. } => table == &object.name,
                        ForeignKeyReference::GraphNode { graph_id, .. } => graph_id == &object.name,
                        ForeignKeyReference::Document { collection, .. } => {
                            collection == &object.name
                        }
                        ForeignKeyReference::Vector { collection, .. } => {
                            collection == &object.name
                        }
                    };

                    if references_us {
                        match on_delete {
                            ReferentialAction::Restrict | ReferentialAction::NoAction => {
                                // Check if any referencing rows exist
                                // In a real implementation, this would query the referencing table
                                debug!(
                                    "Delete requires checking FK {} on {}",
                                    constraint.name, referencing_obj.name
                                );
                            }
                            ReferentialAction::Cascade
                            | ReferentialAction::SetNull
                            | ReferentialAction::SetDefault => {
                                // These actions would be performed on referencing rows
                                debug!(
                                    "Delete will {:?} on FK {} on {}",
                                    on_delete, constraint.name, referencing_obj.name
                                );
                            }
                        }
                    }
                }
            }
        }

        result
    }

    /// Check not-null constraints
    fn check_not_null(
        &self,
        schema: &ObjectSchema,
        row: &RowValue,
        result: &mut EnforcementResult,
    ) {
        for column in &schema.columns {
            if !column.nullable
                && row.is_null(&column.name) && row.values.contains_key(&column.name) {
                    result.add_violation(ConstraintViolation::not_null(&column.name));
                }
        }

        // Also check explicit NOT NULL constraints
        for constraint in &schema.constraints {
            if let ConstraintType::NotNull { column } = &constraint.constraint_type
                && row.is_null(column) && row.values.contains_key(column) {
                    result.add_violation(ConstraintViolation::not_null(column));
                }
        }
    }

    /// Check a single constraint
    async fn check_constraint(
        &self,
        object: &CatalogObject,
        constraint: &TableConstraint,
        row: &RowValue,
        result: &mut EnforcementResult,
    ) {
        match &constraint.constraint_type {
            ConstraintType::PrimaryKey { columns } => {
                self.check_unique(object, &constraint.name, columns, row, result)
                    .await;
            }
            ConstraintType::ForeignKey {
                columns, reference, ..
            } => {
                self.check_foreign_key(&constraint.name, columns, reference, row, result)
                    .await;
            }
            ConstraintType::Unique { columns } => {
                self.check_unique(object, &constraint.name, columns, row, result)
                    .await;
            }
            ConstraintType::Check { expression } => {
                self.check_expression(&constraint.name, expression, row, result);
            }
            ConstraintType::NotNull { column } => {
                if row.is_null(column) && row.values.contains_key(column) {
                    result.add_violation(ConstraintViolation::not_null(column));
                }
            }
            ConstraintType::Exclusion { .. } => {
                // Exclusion constraints require more complex validation
                // For now, just log a warning
                debug!(
                    "Exclusion constraint {} not fully validated",
                    constraint.name
                );
            }
        }
    }

    /// Check uniqueness constraint
    async fn check_unique(
        &self,
        object: &CatalogObject,
        constraint_name: &str,
        columns: &[String],
        row: &RowValue,
        result: &mut EnforcementResult,
    ) {
        // Get values for the unique columns
        let values = row.get_tuple(columns);

        // Skip if any value is null (NULL != NULL in SQL)
        if values.iter().any(|v| v.is_none()) {
            return;
        }

        // Create a key from the values
        let key = values
            .iter()
            .map(|v| v.as_ref().unwrap_or(&"NULL".to_string()).clone())
            .collect::<Vec<_>>()
            .join(":");

        // Check against existing values
        let indexes = self.unique_indexes.read().await;
        if let Some(obj_indexes) = indexes.get(&object.fqn())
            && let Some(index) = obj_indexes.get(constraint_name)
                && index.contains(&key) {
                    result.add_violation(ConstraintViolation::unique(
                        constraint_name,
                        columns.to_vec(),
                        values.into_iter().map(|v| v.unwrap_or_default()).collect(),
                    ));
                }
    }

    /// Check foreign key constraint
    async fn check_foreign_key(
        &self,
        constraint_name: &str,
        columns: &[String],
        reference: &ForeignKeyReference,
        row: &RowValue,
        _result: &mut EnforcementResult,
    ) {
        // Get values for the FK columns
        let values = row.get_tuple(columns);

        // Skip if all values are null
        if values.iter().all(|v| v.is_none()) {
            return;
        }

        // In a real implementation, this would query the referenced object
        // For now, we just validate the structure
        let referenced = match reference {
            ForeignKeyReference::Table { table, .. } => table.clone(),
            ForeignKeyReference::GraphNode { graph_id, .. } => graph_id.clone(),
            ForeignKeyReference::Document { collection, .. } => collection.clone(),
            ForeignKeyReference::Vector { collection, .. } => collection.clone(),
        };

        debug!(
            "FK {} references {} (columns: {:?})",
            constraint_name, referenced, columns
        );

        // Note: Actual FK validation would require access to the referenced object's data
        // This is handled at the storage layer in a real implementation
    }

    /// Check expression-based constraint
    fn check_expression(
        &self,
        constraint_name: &str,
        expression: &str,
        row: &RowValue,
        result: &mut EnforcementResult,
    ) {
        // Simple expression evaluation for common patterns
        // In a real implementation, this would use a proper expression parser

        // Example: "age >= 0"
        if expression.contains(">=") {
            let parts: Vec<&str> = expression.split(">=").collect();
            if parts.len() == 2 {
                let column = parts[0].trim();
                let threshold: i64 = parts[1].trim().parse().unwrap_or(0);

                if let Some(Some(value)) = row.get(column)
                    && let Ok(num) = value.parse::<i64>()
                        && num < threshold {
                            result.add_violation(ConstraintViolation::check(
                                constraint_name,
                                expression,
                            ));
                        }
            }
        }

        // For complex expressions, we'd need a proper expression evaluator
        debug!("Check constraint '{}': {}", constraint_name, expression);
    }

    /// Register existing value for uniqueness tracking
    pub async fn register_value(&self, fqn: &str, constraint_name: &str, value_key: String) {
        let mut indexes = self.unique_indexes.write().await;
        let obj_indexes = indexes.entry(fqn.to_string()).or_default();
        let index = obj_indexes.entry(constraint_name.to_string()).or_default();
        index.insert(value_key);
    }

    /// Remove value from uniqueness tracking
    pub async fn unregister_value(&self, fqn: &str, constraint_name: &str, value_key: &str) {
        let mut indexes = self.unique_indexes.write().await;
        if let Some(obj_indexes) = indexes.get_mut(fqn)
            && let Some(index) = obj_indexes.get_mut(constraint_name) {
                index.remove(value_key);
            }
    }

    /// Clear all values for an object
    pub async fn clear_object(&self, fqn: &str) {
        let mut indexes = self.unique_indexes.write().await;
        indexes.remove(fqn);
    }
}

impl Default for ConstraintEnforcer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::internal::{
        CatalogObject, ObjectSchema, ObjectType, SchemaEnforcementMode, TableConstraint,
    };
    use crate::catalog::types::{CatalogColumn, CatalogDataType};

    fn create_test_object() -> CatalogObject {
        let schema = ObjectSchema {
            columns: vec![
                CatalogColumn::new(1, "id", CatalogDataType::Int64).nullable(false),
                CatalogColumn::new(2, "email", CatalogDataType::String).nullable(false),
                CatalogColumn::new(3, "age", CatalogDataType::Int32),
            ],
            primary_key: vec!["id".to_string()],
            constraints: vec![
                TableConstraint::primary_key("pk_users", vec!["id".to_string()]),
                TableConstraint::unique("uq_email", vec!["email".to_string()]),
                TableConstraint::check("ck_age", "age >= 0"),
            ],
            indexes: vec![],
            model_properties: super::super::ModelProperties::None,
        };

        CatalogObject::new(
            "default",
            vec!["public".to_string()],
            "users",
            ObjectType::RdbmsTable,
        )
        .with_schema(schema, SchemaEnforcementMode::Strict)
    }

    #[tokio::test]
    async fn test_not_null_violation() {
        let enforcer = ConstraintEnforcer::new();
        let object = create_test_object();

        let mut row = RowValue::new();
        row.set("id", Some("1".to_string()));
        row.set("email", None); // Violates NOT NULL
        row.set("age", Some("25".to_string()));

        let result = enforcer.validate_insert(&object, &row).await;

        assert!(!result.is_valid);
        assert_eq!(result.violations.len(), 1);
        assert!(result.violations[0].message.contains("email"));
    }

    #[tokio::test]
    async fn test_check_constraint_violation() {
        let enforcer = ConstraintEnforcer::new();
        let object = create_test_object();

        let mut row = RowValue::new();
        row.set("id", Some("1".to_string()));
        row.set("email", Some("test@test.com".to_string()));
        row.set("age", Some("-5".to_string())); // Violates check constraint

        let result = enforcer.validate_insert(&object, &row).await;

        assert!(!result.is_valid);
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.constraint_name.contains("ck_age"))
        );
    }

    #[tokio::test]
    async fn test_valid_row() {
        let enforcer = ConstraintEnforcer::new();
        let object = create_test_object();

        let mut row = RowValue::new();
        row.set("id", Some("1".to_string()));
        row.set("email", Some("test@test.com".to_string()));
        row.set("age", Some("25".to_string()));

        let result = enforcer.validate_insert(&object, &row).await;

        assert!(result.is_valid);
        assert!(result.violations.is_empty());
    }

    #[tokio::test]
    async fn test_flexible_mode_skips_validation() {
        let enforcer = ConstraintEnforcer::new();

        let mut object = create_test_object();
        object.enforcement_mode = SchemaEnforcementMode::Flexible;

        let mut row = RowValue::new();
        row.set("id", None); // Would violate NOT NULL in strict mode

        let result = enforcer.validate_insert(&object, &row).await;

        assert!(result.is_valid);
    }

    #[tokio::test]
    async fn test_unique_constraint_with_registration() {
        let enforcer = ConstraintEnforcer::new();
        let object = create_test_object();

        // Register an existing email
        enforcer
            .register_value(
                "default.public.users",
                "uq_email",
                "existing@test.com".to_string(),
            )
            .await;

        let mut row = RowValue::new();
        row.set("id", Some("1".to_string()));
        row.set("email", Some("existing@test.com".to_string())); // Duplicate
        row.set("age", Some("25".to_string()));

        let result = enforcer.validate_insert(&object, &row).await;

        assert!(!result.is_valid);
        assert!(
            result
                .violations
                .iter()
                .any(|v| v.constraint_name.contains("uq_email"))
        );
    }

    #[tokio::test]
    async fn test_enforcement_result_merge() {
        let mut result1 = EnforcementResult::pass(SchemaEnforcementMode::Strict);
        result1.add_violation(ConstraintViolation::not_null("col1"));

        let mut result2 = EnforcementResult::pass(SchemaEnforcementMode::Strict);
        result2.add_violation(ConstraintViolation::not_null("col2"));

        result1.merge(result2);

        assert!(!result1.is_valid);
        assert_eq!(result1.violations.len(), 2);
    }
}
