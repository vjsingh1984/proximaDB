// PostgreSQL initial snapshot handler
//
// Captures consistent initial state of configured tables via
// COPY commands within a single transaction snapshot.

use super::config::{PostgresConfig, SnapshotMode, TableConfig};

/// Handles initial table snapshot for CDC.
pub struct SnapshotHandler {
    config: PostgresConfig,
}

/// A snapshot row from a table.
#[derive(Debug, Clone)]
pub struct SnapshotRow {
    /// Schema name (e.g., "public")
    pub schema: String,
    /// Table name
    pub table: String,
    /// Column names in the snapshot
    pub columns: Vec<String>,
    /// Column values as strings
    pub values: Vec<String>,
}

/// Result of a snapshot operation.
#[derive(Debug, Clone)]
pub struct SnapshotResult {
    /// Captured rows from the snapshot
    pub rows: Vec<SnapshotRow>,
    /// Consistent LSN position for the snapshot
    pub consistent_lsn: u64,
    /// Number of tables captured in the snapshot
    pub tables_captured: usize,
}

impl SnapshotHandler {
    /// Create a new snapshot handler with the given configuration.
    pub fn new(config: PostgresConfig) -> Self {
        Self { config }
    }

    /// Whether a snapshot should be taken based on config.
    pub fn should_snapshot(&self) -> bool {
        matches!(self.config.snapshot_mode, SnapshotMode::Initial)
    }

    /// Generate the SQL to start a consistent snapshot transaction.
    pub fn begin_snapshot_sql() -> &'static str {
        "BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY"
    }

    /// Generate the SQL to get the current WAL position for consistency.
    pub fn get_consistent_point_sql() -> &'static str {
        "SELECT pg_current_wal_lsn()::text"
    }

    /// Generate the COPY command for a table.
    pub fn copy_table_sql(table_config: &TableConfig) -> String {
        let columns: Vec<&str> = table_config
            .primary_key
            .iter()
            .chain(table_config.metadata_columns.iter())
            .chain(table_config.vector_column.iter())
            .map(|s| s.as_str())
            .collect();

        if columns.is_empty() {
            format!(
                "COPY \"{}\".\"{}\" TO STDOUT WITH (FORMAT csv, HEADER true)",
                table_config.schema, table_config.table
            )
        } else {
            format!(
                "COPY \"{}\".\"{}\" ({}) TO STDOUT WITH (FORMAT csv, HEADER true)",
                table_config.schema,
                table_config.table,
                columns.join(", ")
            )
        }
    }

    /// Get all tables configured for snapshot.
    pub fn tables(&self) -> &[TableConfig] {
        &self.config.tables
    }

    /// Snapshot mode.
    pub fn mode(&self) -> &SnapshotMode {
        &self.config.snapshot_mode
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::SlotBehavior;
    use super::*;
    use std::time::Duration;

    fn test_table() -> TableConfig {
        TableConfig {
            schema: "public".to_string(),
            table: "products".to_string(),
            primary_key: vec!["id".to_string()],
            vector_column: Some("embedding".to_string()),
            embed_columns: Some(vec!["title".to_string()]),
            metadata_columns: vec!["category".to_string(), "price".to_string()],
            column_mappings: Vec::new(),
        }
    }

    fn test_config(mode: SnapshotMode) -> PostgresConfig {
        PostgresConfig {
            connection_string: "postgres://test:test@localhost/testdb".to_string(),
            slot_name: "test_slot".to_string(),
            publication: "test_pub".to_string(),
            tables: vec![test_table()],
            snapshot_mode: mode,
            connect_timeout: Duration::from_secs(10),
            heartbeat_interval: Duration::from_secs(10),
            batch_size: 1000,
            slot_behavior: SlotBehavior::CreateIfNotExists,
        }
    }

    #[test]
    fn test_should_snapshot() {
        let handler = SnapshotHandler::new(test_config(SnapshotMode::Initial));
        assert!(handler.should_snapshot());

        let handler_skip = SnapshotHandler::new(test_config(SnapshotMode::Never));
        assert!(!handler_skip.should_snapshot());
    }

    #[test]
    fn test_copy_table_sql() {
        let table = test_table();
        let sql = SnapshotHandler::copy_table_sql(&table);
        assert!(sql.contains("COPY \"public\".\"products\""));
        assert!(sql.contains("id"));
        assert!(sql.contains("embedding"));
        assert!(sql.contains("FORMAT csv"));
    }

    #[test]
    fn test_begin_snapshot_sql() {
        let sql = SnapshotHandler::begin_snapshot_sql();
        assert!(sql.contains("REPEATABLE READ"));
        assert!(sql.contains("READ ONLY"));
    }

    #[test]
    fn test_consistent_point_sql() {
        let sql = SnapshotHandler::get_consistent_point_sql();
        assert!(sql.contains("pg_current_wal_lsn"));
    }
}
