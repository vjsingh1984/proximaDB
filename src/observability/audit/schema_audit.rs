/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Schema change audit logging
//!
//! Tracks schema changes for compliance and debugging. This module provides
//! comprehensive audit logging for all schema modifications in ProximaDB.
//!
//! # Features
//!
//! - Track schema creation, modification, and deletion
//! - Record column additions, removals, and modifications
//! - Capture enforcement mode changes
//! - Support schema migration tracking
//! - Maintain change history with before/after states
//!
//! # Usage
//!
//! ```rust,ignore
//! use proximadb::observability::audit::{SchemaAuditLogger, SchemaAuditRecord, SchemaChangeType};
//! use chrono::Utc;
//!
//! let mut logger = SchemaAuditLogger::new(1000);
//!
//! // Log a schema creation
//! logger.log_change(SchemaAuditRecord {
//!     timestamp: Utc::now(),
//!     collection_name: "products".to_string(),
//!     change_type: SchemaChangeType::Created,
//!     before_schema: None,
//!     after_schema: Some(r#"{"columns": [...]}"#.to_string()),
//!     user: Some("admin".to_string()),
//!     reason: Some("Initial schema creation".to_string()),
//! });
//!
//! // Query history for a collection
//! let history = logger.get_history("products");
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Type of schema change
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SchemaChangeType {
    /// Schema was created
    Created,
    /// Column was added to schema
    ColumnAdded,
    /// Column was removed from schema
    ColumnRemoved,
    /// Column definition was modified
    ColumnModified,
    /// Schema enforcement mode was changed
    EnforcementChanged,
    /// Schema was migrated to new version
    Migrated,
    /// Schema was deleted
    Deleted,
    /// Column was renamed
    ColumnRenamed,
    /// Index was added
    IndexAdded,
    /// Index was removed
    IndexRemoved,
    /// Constraint was added
    ConstraintAdded,
    /// Constraint was removed
    ConstraintRemoved,
}

impl std::fmt::Display for SchemaChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaChangeType::Created => write!(f, "CREATED"),
            SchemaChangeType::ColumnAdded => write!(f, "COLUMN_ADDED"),
            SchemaChangeType::ColumnRemoved => write!(f, "COLUMN_REMOVED"),
            SchemaChangeType::ColumnModified => write!(f, "COLUMN_MODIFIED"),
            SchemaChangeType::EnforcementChanged => write!(f, "ENFORCEMENT_CHANGED"),
            SchemaChangeType::Migrated => write!(f, "MIGRATED"),
            SchemaChangeType::Deleted => write!(f, "DELETED"),
            SchemaChangeType::ColumnRenamed => write!(f, "COLUMN_RENAMED"),
            SchemaChangeType::IndexAdded => write!(f, "INDEX_ADDED"),
            SchemaChangeType::IndexRemoved => write!(f, "INDEX_REMOVED"),
            SchemaChangeType::ConstraintAdded => write!(f, "CONSTRAINT_ADDED"),
            SchemaChangeType::ConstraintRemoved => write!(f, "CONSTRAINT_REMOVED"),
        }
    }
}

/// Schema change audit record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaAuditRecord {
    /// Timestamp of the change
    pub timestamp: DateTime<Utc>,
    /// Name of the collection whose schema changed
    pub collection_name: String,
    /// Type of schema change
    pub change_type: SchemaChangeType,
    /// Schema state before the change (JSON serialized)
    pub before_schema: Option<String>,
    /// Schema state after the change (JSON serialized)
    pub after_schema: Option<String>,
    /// User who made the change
    pub user: Option<String>,
    /// Reason for the change
    pub reason: Option<String>,
}

impl SchemaAuditRecord {
    /// Create a new schema audit record
    pub fn new(collection_name: String, change_type: SchemaChangeType) -> Self {
        Self {
            timestamp: Utc::now(),
            collection_name,
            change_type,
            before_schema: None,
            after_schema: None,
            user: None,
            reason: None,
        }
    }

    /// Set the before schema state
    pub fn with_before_schema(mut self, schema: String) -> Self {
        self.before_schema = Some(schema);
        self
    }

    /// Set the after schema state
    pub fn with_after_schema(mut self, schema: String) -> Self {
        self.after_schema = Some(schema);
        self
    }

    /// Set the user who made the change
    pub fn with_user(mut self, user: String) -> Self {
        self.user = Some(user);
        self
    }

    /// Set the reason for the change
    pub fn with_reason(mut self, reason: String) -> Self {
        self.reason = Some(reason);
        self
    }

    /// Set the timestamp
    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }
}

/// Schema audit logger
///
/// Maintains an in-memory log of schema changes with configurable maximum size.
/// When the maximum size is exceeded, oldest records are removed.
pub struct SchemaAuditLogger {
    /// Audit records (newest first)
    records: VecDeque<SchemaAuditRecord>,
    /// Maximum number of records to keep
    max_records: usize,
}

impl SchemaAuditLogger {
    /// Create a new schema audit logger
    ///
    /// # Arguments
    ///
    /// * `max_records` - Maximum number of records to keep in memory
    pub fn new(max_records: usize) -> Self {
        Self {
            records: VecDeque::with_capacity(max_records.min(10000)),
            max_records,
        }
    }

    /// Log a schema change
    pub fn log_change(&mut self, record: SchemaAuditRecord) {
        // Add to front (newest first)
        self.records.push_front(record);

        // Remove oldest if over capacity
        while self.records.len() > self.max_records {
            self.records.pop_back();
        }
    }

    /// Log a schema creation
    pub fn log_created(&mut self, collection_name: &str, schema_json: &str, user: Option<&str>) {
        let mut record =
            SchemaAuditRecord::new(collection_name.to_string(), SchemaChangeType::Created)
                .with_after_schema(schema_json.to_string());

        if let Some(u) = user {
            record = record.with_user(u.to_string());
        }

        self.log_change(record);
    }

    /// Log a column addition
    pub fn log_column_added(
        &mut self,
        collection_name: &str,
        column_name: &str,
        before_schema: &str,
        after_schema: &str,
        user: Option<&str>,
    ) {
        let mut record =
            SchemaAuditRecord::new(collection_name.to_string(), SchemaChangeType::ColumnAdded)
                .with_before_schema(before_schema.to_string())
                .with_after_schema(after_schema.to_string())
                .with_reason(format!("Added column: {}", column_name));

        if let Some(u) = user {
            record = record.with_user(u.to_string());
        }

        self.log_change(record);
    }

    /// Log a column removal
    pub fn log_column_removed(
        &mut self,
        collection_name: &str,
        column_name: &str,
        before_schema: &str,
        after_schema: &str,
        user: Option<&str>,
    ) {
        let mut record =
            SchemaAuditRecord::new(collection_name.to_string(), SchemaChangeType::ColumnRemoved)
                .with_before_schema(before_schema.to_string())
                .with_after_schema(after_schema.to_string())
                .with_reason(format!("Removed column: {}", column_name));

        if let Some(u) = user {
            record = record.with_user(u.to_string());
        }

        self.log_change(record);
    }

    /// Log a schema migration
    pub fn log_migrated(
        &mut self,
        collection_name: &str,
        from_version: &str,
        to_version: &str,
        before_schema: &str,
        after_schema: &str,
        user: Option<&str>,
    ) {
        let mut record =
            SchemaAuditRecord::new(collection_name.to_string(), SchemaChangeType::Migrated)
                .with_before_schema(before_schema.to_string())
                .with_after_schema(after_schema.to_string())
                .with_reason(format!("Migration from {} to {}", from_version, to_version));

        if let Some(u) = user {
            record = record.with_user(u.to_string());
        }

        self.log_change(record);
    }

    /// Log a schema deletion
    pub fn log_deleted(&mut self, collection_name: &str, before_schema: &str, user: Option<&str>) {
        let mut record =
            SchemaAuditRecord::new(collection_name.to_string(), SchemaChangeType::Deleted)
                .with_before_schema(before_schema.to_string());

        if let Some(u) = user {
            record = record.with_user(u.to_string());
        }

        self.log_change(record);
    }

    /// Log an enforcement mode change
    pub fn log_enforcement_changed(
        &mut self,
        collection_name: &str,
        from_mode: &str,
        to_mode: &str,
        user: Option<&str>,
    ) {
        let mut record = SchemaAuditRecord::new(
            collection_name.to_string(),
            SchemaChangeType::EnforcementChanged,
        )
        .with_reason(format!(
            "Changed enforcement from {} to {}",
            from_mode, to_mode
        ));

        if let Some(u) = user {
            record = record.with_user(u.to_string());
        }

        self.log_change(record);
    }

    /// Get history for a specific collection
    pub fn get_history(&self, collection_name: &str) -> Vec<&SchemaAuditRecord> {
        self.records
            .iter()
            .filter(|r| r.collection_name == collection_name)
            .collect()
    }

    /// Get recent records across all collections
    pub fn get_recent(&self, limit: usize) -> Vec<&SchemaAuditRecord> {
        self.records.iter().take(limit).collect()
    }

    /// Get records within a time range
    pub fn get_by_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<&SchemaAuditRecord> {
        self.records
            .iter()
            .filter(|r| r.timestamp >= start && r.timestamp <= end)
            .collect()
    }

    /// Get records by change type
    pub fn get_by_change_type(&self, change_type: SchemaChangeType) -> Vec<&SchemaAuditRecord> {
        self.records
            .iter()
            .filter(|r| r.change_type == change_type)
            .collect()
    }

    /// Get records by user
    pub fn get_by_user(&self, user: &str) -> Vec<&SchemaAuditRecord> {
        self.records
            .iter()
            .filter(|r| r.user.as_deref() == Some(user))
            .collect()
    }

    /// Get total record count
    pub fn count(&self) -> usize {
        self.records.len()
    }

    /// Clear all records
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.records.clear();
    }

    /// Export records as JSON
    pub fn export_json(&self) -> Result<String, serde_json::Error> {
        let records: Vec<&SchemaAuditRecord> = self.records.iter().collect();
        serde_json::to_string_pretty(&records)
    }

    /// Export records for a specific collection as JSON
    pub fn export_collection_json(
        &self,
        collection_name: &str,
    ) -> Result<String, serde_json::Error> {
        let records = self.get_history(collection_name);
        serde_json::to_string_pretty(&records)
    }
}

impl Default for SchemaAuditLogger {
    fn default() -> Self {
        Self::new(10000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_schema_audit_logger_creation() {
        let logger = SchemaAuditLogger::new(100);
        assert_eq!(logger.count(), 0);
    }

    #[test]
    fn test_log_schema_creation() {
        let mut logger = SchemaAuditLogger::new(100);

        logger.log_created("products", r#"{"columns": []}"#, Some("admin"));

        assert_eq!(logger.count(), 1);

        let history = logger.get_history("products");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].change_type, SchemaChangeType::Created);
        assert_eq!(history[0].user.as_deref(), Some("admin"));
    }

    #[test]
    fn test_log_column_operations() {
        let mut logger = SchemaAuditLogger::new(100);

        logger.log_column_added(
            "products",
            "price",
            r#"{"columns": []}"#,
            r#"{"columns": ["price"]}"#,
            Some("developer"),
        );

        logger.log_column_removed(
            "products",
            "old_field",
            r#"{"columns": ["old_field"]}"#,
            r#"{"columns": []}"#,
            Some("developer"),
        );

        assert_eq!(logger.count(), 2);

        let history = logger.get_history("products");
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_max_records_limit() {
        let mut logger = SchemaAuditLogger::new(3);

        for i in 0..5 {
            logger.log_created(&format!("collection_{}", i), "{}", None);
        }

        // Should only keep 3 most recent
        assert_eq!(logger.count(), 3);

        // Most recent should be collection_4
        let recent = logger.get_recent(1);
        assert_eq!(recent[0].collection_name, "collection_4");
    }

    #[test]
    fn test_get_by_time_range() {
        let mut logger = SchemaAuditLogger::new(100);

        // Create records at different times
        let now = Utc::now();

        let old_record =
            SchemaAuditRecord::new("old_collection".to_string(), SchemaChangeType::Created)
                .with_timestamp(now - Duration::hours(2));

        let recent_record =
            SchemaAuditRecord::new("recent_collection".to_string(), SchemaChangeType::Created)
                .with_timestamp(now);

        logger.log_change(old_record);
        logger.log_change(recent_record);

        // Query last hour
        let results =
            logger.get_by_time_range(now - Duration::hours(1), now + Duration::minutes(1));

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].collection_name, "recent_collection");
    }

    #[test]
    fn test_get_by_change_type() {
        let mut logger = SchemaAuditLogger::new(100);

        logger.log_created("col1", "{}", None);
        logger.log_deleted("col2", "{}", None);
        logger.log_created("col3", "{}", None);

        let created = logger.get_by_change_type(SchemaChangeType::Created);
        assert_eq!(created.len(), 2);

        let deleted = logger.get_by_change_type(SchemaChangeType::Deleted);
        assert_eq!(deleted.len(), 1);
    }

    #[test]
    fn test_get_by_user() {
        let mut logger = SchemaAuditLogger::new(100);

        logger.log_created("col1", "{}", Some("alice"));
        logger.log_created("col2", "{}", Some("bob"));
        logger.log_created("col3", "{}", Some("alice"));

        let alice_changes = logger.get_by_user("alice");
        assert_eq!(alice_changes.len(), 2);

        let bob_changes = logger.get_by_user("bob");
        assert_eq!(bob_changes.len(), 1);
    }

    #[test]
    fn test_export_json() {
        let mut logger = SchemaAuditLogger::new(100);

        logger.log_created("products", r#"{"columns": ["id", "name"]}"#, Some("admin"));

        let json = logger.export_json().expect("Failed to export JSON");
        assert!(json.contains("products"));
        assert!(json.contains("CREATED"));
    }

    #[test]
    fn test_clear() {
        let mut logger = SchemaAuditLogger::new(100);

        logger.log_created("col1", "{}", None);
        logger.log_created("col2", "{}", None);

        assert_eq!(logger.count(), 2);

        logger.clear();
        assert_eq!(logger.count(), 0);
    }

    #[test]
    fn test_schema_change_type_display() {
        assert_eq!(SchemaChangeType::Created.to_string(), "CREATED");
        assert_eq!(SchemaChangeType::ColumnAdded.to_string(), "COLUMN_ADDED");
        assert_eq!(
            SchemaChangeType::ColumnRemoved.to_string(),
            "COLUMN_REMOVED"
        );
        assert_eq!(
            SchemaChangeType::EnforcementChanged.to_string(),
            "ENFORCEMENT_CHANGED"
        );
        assert_eq!(SchemaChangeType::Migrated.to_string(), "MIGRATED");
    }

    #[test]
    fn test_schema_audit_record_builder() {
        let record =
            SchemaAuditRecord::new("test_collection".to_string(), SchemaChangeType::Created)
                .with_before_schema(r#"{"version": 1}"#.to_string())
                .with_after_schema(r#"{"version": 2}"#.to_string())
                .with_user("test_user".to_string())
                .with_reason("Test migration".to_string());

        assert_eq!(record.collection_name, "test_collection");
        assert_eq!(record.change_type, SchemaChangeType::Created);
        assert_eq!(record.before_schema.as_deref(), Some(r#"{"version": 1}"#));
        assert_eq!(record.after_schema.as_deref(), Some(r#"{"version": 2}"#));
        assert_eq!(record.user.as_deref(), Some("test_user"));
        assert_eq!(record.reason.as_deref(), Some("Test migration"));
    }

    #[test]
    fn test_log_enforcement_changed() {
        let mut logger = SchemaAuditLogger::new(100);

        logger.log_enforcement_changed("my_collection", "Flexible", "Strict", Some("admin"));

        let history = logger.get_history("my_collection");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].change_type, SchemaChangeType::EnforcementChanged);
        assert!(
            history[0]
                .reason
                .as_ref()
                .expect("reason should exist")
                .contains("Flexible")
        );
    }

    #[test]
    fn test_log_migrated() {
        let mut logger = SchemaAuditLogger::new(100);

        logger.log_migrated(
            "products",
            "1.0.0",
            "2.0.0",
            r#"{"version": "1.0.0"}"#,
            r#"{"version": "2.0.0"}"#,
            Some("migration_bot"),
        );

        let history = logger.get_history("products");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].change_type, SchemaChangeType::Migrated);
        assert!(
            history[0]
                .reason
                .as_ref()
                .expect("reason should exist")
                .contains("1.0.0 to 2.0.0")
        );
    }
}
