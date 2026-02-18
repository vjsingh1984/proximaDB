/*
 * Copyright 2025 Vijaykumar Singh
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

//! PostgreSQL connector configuration

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// PostgreSQL connector configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresConfig {
    /// PostgreSQL connection string
    /// Format: postgres://user:password@host:port/database
    pub connection_string: String,

    /// Name of the replication slot
    /// Will be created if it doesn't exist
    pub slot_name: String,

    /// PostgreSQL publication name
    /// Must be created in PostgreSQL before use
    pub publication: String,

    /// Tables to capture changes from
    pub tables: Vec<TableConfig>,

    /// Snapshot mode for initial data load
    pub snapshot_mode: SnapshotMode,

    /// Connection timeout
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: Duration,

    /// Heartbeat interval for replication keepalive
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: Duration,

    /// Maximum batch size for events
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    /// Slot creation behavior
    #[serde(default)]
    pub slot_behavior: SlotBehavior,
}

fn default_connect_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_heartbeat_interval() -> Duration {
    Duration::from_secs(10)
}

fn default_batch_size() -> usize {
    1000
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            connection_string: String::new(),
            slot_name: "proximadb_cdc".to_string(),
            publication: "proximadb_pub".to_string(),
            tables: Vec::new(),
            snapshot_mode: SnapshotMode::Initial,
            connect_timeout: default_connect_timeout(),
            heartbeat_interval: default_heartbeat_interval(),
            batch_size: default_batch_size(),
            slot_behavior: SlotBehavior::default(),
        }
    }
}

impl PostgresConfig {
    /// Create a new configuration with connection string
    pub fn new(connection_string: impl Into<String>) -> Self {
        Self {
            connection_string: connection_string.into(),
            ..Default::default()
        }
    }

    /// Set slot name
    pub fn with_slot(mut self, slot_name: impl Into<String>) -> Self {
        self.slot_name = slot_name.into();
        self
    }

    /// Set publication name
    pub fn with_publication(mut self, publication: impl Into<String>) -> Self {
        self.publication = publication.into();
        self
    }

    /// Add a table configuration
    pub fn with_table(mut self, table: TableConfig) -> Self {
        self.tables.push(table);
        self
    }

    /// Set snapshot mode
    pub fn with_snapshot_mode(mut self, mode: SnapshotMode) -> Self {
        self.snapshot_mode = mode;
        self
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.connection_string.is_empty() {
            return Err("Connection string is required".to_string());
        }
        if self.slot_name.is_empty() {
            return Err("Slot name is required".to_string());
        }
        if self.publication.is_empty() {
            return Err("Publication name is required".to_string());
        }
        Ok(())
    }

    /// Get fully qualified table name
    pub fn get_table(&self, schema: &str, table: &str) -> Option<&TableConfig> {
        self.tables
            .iter()
            .find(|t| t.schema == schema && t.table == table)
    }
}

/// Configuration for a single table
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableConfig {
    /// Schema name (e.g., "public")
    pub schema: String,

    /// Table name
    pub table: String,

    /// Primary key columns
    pub primary_key: Vec<String>,

    /// Column containing vector data (if pre-computed)
    pub vector_column: Option<String>,

    /// Columns to embed into vectors (for automatic embedding)
    pub embed_columns: Option<Vec<String>>,

    /// Columns to include as metadata
    pub metadata_columns: Vec<String>,

    /// Column mappings for transformation
    #[serde(default)]
    pub column_mappings: Vec<ColumnMapping>,
}

impl TableConfig {
    /// Create a new table configuration
    pub fn new(schema: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            table: table.into(),
            primary_key: Vec::new(),
            vector_column: None,
            embed_columns: None,
            metadata_columns: Vec::new(),
            column_mappings: Vec::new(),
        }
    }

    /// Set primary key columns
    pub fn with_primary_key(mut self, columns: Vec<String>) -> Self {
        self.primary_key = columns;
        self
    }

    /// Set vector column
    pub fn with_vector_column(mut self, column: impl Into<String>) -> Self {
        self.vector_column = Some(column.into());
        self
    }

    /// Set embed columns
    pub fn with_embed_columns(mut self, columns: Vec<String>) -> Self {
        self.embed_columns = Some(columns);
        self
    }

    /// Set metadata columns
    pub fn with_metadata_columns(mut self, columns: Vec<String>) -> Self {
        self.metadata_columns = columns;
        self
    }

    /// Get fully qualified table name
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.schema, self.table)
    }
}

/// Column mapping for transformation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMapping {
    /// Source column name in PostgreSQL
    pub source: String,
    /// Target field name in ProximaDB
    pub target: String,
    /// Transformation to apply
    #[serde(default)]
    pub transform: ColumnTransform,
}

/// Column transformation types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColumnTransform {
    /// No transformation (copy as-is)
    #[default]
    None,
    /// Parse as JSON
    Json,
    /// Convert to string
    ToString,
    /// Parse vector from array or JSON
    Vector,
    /// Parse timestamp
    Timestamp,
}

/// Snapshot mode for initial data load
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    /// Take snapshot only if no offset exists
    #[default]
    Initial,
    /// Always take a new snapshot on start
    Always,
    /// Never take a snapshot, start from current position
    Never,
    /// Export snapshot to file before streaming
    Export,
}

/// Replication slot behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SlotBehavior {
    /// Create slot if it doesn't exist
    #[default]
    CreateIfNotExists,
    /// Require slot to exist
    RequireExisting,
    /// Create temporary slot (dropped on disconnect)
    Temporary,
    /// Drop and recreate slot on each start
    DropAndCreate,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgres_config_default() {
        let config = PostgresConfig::default();
        assert_eq!(config.slot_name, "proximadb_cdc");
        assert_eq!(config.publication, "proximadb_pub");
        assert!(config.tables.is_empty());
    }

    #[test]
    fn test_postgres_config_builder() {
        let config = PostgresConfig::new("postgres://localhost/db")
            .with_slot("my_slot")
            .with_publication("my_pub")
            .with_snapshot_mode(SnapshotMode::Never)
            .with_table(
                TableConfig::new("public", "users")
                    .with_primary_key(vec!["id".to_string()])
                    .with_vector_column("embedding"),
            );

        assert_eq!(config.connection_string, "postgres://localhost/db");
        assert_eq!(config.slot_name, "my_slot");
        assert_eq!(config.publication, "my_pub");
        assert_eq!(config.snapshot_mode, SnapshotMode::Never);
        assert_eq!(config.tables.len(), 1);
    }

    #[test]
    fn test_config_validation() {
        let config = PostgresConfig::default();
        assert!(config.validate().is_err());

        let config = PostgresConfig::new("postgres://localhost/db");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_table_config() {
        let table = TableConfig::new("public", "products")
            .with_primary_key(vec!["id".to_string()])
            .with_vector_column("embedding")
            .with_embed_columns(vec!["title".to_string(), "description".to_string()])
            .with_metadata_columns(vec!["category".to_string(), "price".to_string()]);

        assert_eq!(table.full_name(), "public.products");
        assert!(table.vector_column.is_some());
        assert!(table.embed_columns.is_some());
        assert_eq!(table.embed_columns.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_get_table() {
        let config = PostgresConfig::new("postgres://localhost/db")
            .with_table(TableConfig::new("public", "users"))
            .with_table(TableConfig::new("public", "products"));

        assert!(config.get_table("public", "users").is_some());
        assert!(config.get_table("public", "products").is_some());
        assert!(config.get_table("public", "orders").is_none());
    }

    #[test]
    fn test_config_serialization() {
        let config = PostgresConfig::new("postgres://localhost/db").with_table(
            TableConfig::new("public", "users").with_primary_key(vec!["id".to_string()]),
        );

        let json = serde_json::to_string(&config).unwrap();
        let parsed: PostgresConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.connection_string, config.connection_string);
        assert_eq!(parsed.tables.len(), 1);
    }

    #[test]
    fn test_snapshot_modes() {
        assert_eq!(SnapshotMode::default(), SnapshotMode::Initial);

        let modes = vec![
            SnapshotMode::Initial,
            SnapshotMode::Always,
            SnapshotMode::Never,
            SnapshotMode::Export,
        ];

        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: SnapshotMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn test_slot_behaviors() {
        assert_eq!(SlotBehavior::default(), SlotBehavior::CreateIfNotExists);

        let behaviors = vec![
            SlotBehavior::CreateIfNotExists,
            SlotBehavior::RequireExisting,
            SlotBehavior::Temporary,
            SlotBehavior::DropAndCreate,
        ];

        for behavior in behaviors {
            let json = serde_json::to_string(&behavior).unwrap();
            let parsed: SlotBehavior = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, behavior);
        }
    }
}
