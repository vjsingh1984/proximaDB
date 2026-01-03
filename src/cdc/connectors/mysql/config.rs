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

//! MySQL connector configuration

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// MySQL connector configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MySqlConfig {
    /// MySQL connection URL
    /// Format: mysql://user:password@host:port/database
    pub connection_url: String,

    /// Server ID for binlog replication (must be unique)
    pub server_id: u32,

    /// Tables to capture
    pub tables: Vec<MySqlTableConfig>,

    /// GTID mode configuration
    pub gtid_mode: GtidMode,

    /// Starting position (if not using GTID)
    pub start_position: Option<BinlogPosition>,

    /// Connection timeout
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: Duration,

    /// Read timeout for binlog events
    #[serde(default = "default_read_timeout")]
    pub read_timeout: Duration,

    /// Heartbeat interval
    #[serde(default = "default_heartbeat")]
    pub heartbeat_interval: Duration,

    /// Whether to skip GTID purged check
    #[serde(default)]
    pub skip_gtid_purged_check: bool,

    /// SSL mode
    #[serde(default)]
    pub ssl_mode: SslMode,
}

fn default_connect_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_read_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_heartbeat() -> Duration {
    Duration::from_secs(30)
}

impl Default for MySqlConfig {
    fn default() -> Self {
        Self {
            connection_url: String::new(),
            server_id: 1,
            tables: Vec::new(),
            gtid_mode: GtidMode::Auto,
            start_position: None,
            connect_timeout: default_connect_timeout(),
            read_timeout: default_read_timeout(),
            heartbeat_interval: default_heartbeat(),
            skip_gtid_purged_check: false,
            ssl_mode: SslMode::default(),
        }
    }
}

impl MySqlConfig {
    /// Create a new MySQL configuration
    pub fn new(connection_url: impl Into<String>) -> Self {
        Self {
            connection_url: connection_url.into(),
            ..Default::default()
        }
    }

    /// Set server ID
    pub fn with_server_id(mut self, server_id: u32) -> Self {
        self.server_id = server_id;
        self
    }

    /// Add a table configuration
    pub fn with_table(mut self, table: MySqlTableConfig) -> Self {
        self.tables.push(table);
        self
    }

    /// Set GTID mode
    pub fn with_gtid_mode(mut self, mode: GtidMode) -> Self {
        self.gtid_mode = mode;
        self
    }

    /// Set starting binlog position
    pub fn with_start_position(mut self, position: BinlogPosition) -> Self {
        self.start_position = Some(position);
        self
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.connection_url.is_empty() {
            return Err("Connection URL is required".to_string());
        }
        if self.server_id == 0 {
            return Err("Server ID must be non-zero".to_string());
        }
        Ok(())
    }

    /// Get table configuration by name
    pub fn get_table(&self, database: &str, table: &str) -> Option<&MySqlTableConfig> {
        self.tables
            .iter()
            .find(|t| t.database == database && t.table == table)
    }
}

/// MySQL table configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MySqlTableConfig {
    /// Database name
    pub database: String,

    /// Table name
    pub table: String,

    /// Primary key columns
    pub primary_key: Vec<String>,

    /// Column containing vector data
    pub vector_column: Option<String>,

    /// Columns to embed into vectors
    pub embed_columns: Option<Vec<String>>,

    /// Columns to include as metadata
    pub metadata_columns: Vec<String>,
}

impl MySqlTableConfig {
    /// Create a new table configuration
    pub fn new(database: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            database: database.into(),
            table: table.into(),
            primary_key: Vec::new(),
            vector_column: None,
            embed_columns: None,
            metadata_columns: Vec::new(),
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

    /// Get full table name
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.database, self.table)
    }
}

/// GTID mode configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GtidMode {
    /// Automatically detect GTID support
    #[default]
    Auto,
    /// Force GTID mode
    On,
    /// Disable GTID, use binlog position
    Off,
}

/// Binlog position
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinlogPosition {
    /// Binlog filename
    pub filename: String,
    /// Position in the file
    pub position: u64,
}

impl BinlogPosition {
    /// Create a new binlog position
    pub fn new(filename: impl Into<String>, position: u64) -> Self {
        Self {
            filename: filename.into(),
            position,
        }
    }

    /// Parse from "filename:position" format
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            let position = parts[1].parse().ok()?;
            Some(Self {
                filename: parts[0].to_string(),
                position,
            })
        } else {
            None
        }
    }

    /// Format as "filename:position"
    pub fn format(&self) -> String {
        format!("{}:{}", self.filename, self.position)
    }
}

/// SSL mode for MySQL connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SslMode {
    /// No SSL
    #[default]
    Disabled,
    /// Prefer SSL but allow unencrypted
    Preferred,
    /// Require SSL
    Required,
    /// Verify server certificate
    VerifyCa,
    /// Verify server certificate and hostname
    VerifyIdentity,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mysql_config_default() {
        let config = MySqlConfig::default();
        assert_eq!(config.server_id, 1);
        assert!(config.tables.is_empty());
        assert_eq!(config.gtid_mode, GtidMode::Auto);
    }

    #[test]
    fn test_mysql_config_builder() {
        let config = MySqlConfig::new("mysql://localhost/db")
            .with_server_id(12345)
            .with_gtid_mode(GtidMode::On)
            .with_table(MySqlTableConfig::new("mydb", "users"));

        assert_eq!(config.connection_url, "mysql://localhost/db");
        assert_eq!(config.server_id, 12345);
        assert_eq!(config.gtid_mode, GtidMode::On);
        assert_eq!(config.tables.len(), 1);
    }

    #[test]
    fn test_config_validation() {
        let config = MySqlConfig::default();
        assert!(config.validate().is_err());

        let config = MySqlConfig::new("mysql://localhost/db");
        assert!(config.validate().is_ok());

        let config = MySqlConfig::new("mysql://localhost/db").with_server_id(0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_table_config() {
        let table = MySqlTableConfig::new("mydb", "products")
            .with_primary_key(vec!["id".to_string()])
            .with_vector_column("embedding")
            .with_metadata_columns(vec!["name".to_string(), "price".to_string()]);

        assert_eq!(table.full_name(), "mydb.products");
        assert!(table.vector_column.is_some());
        assert_eq!(table.metadata_columns.len(), 2);
    }

    #[test]
    fn test_binlog_position() {
        let pos = BinlogPosition::new("mysql-bin.000001", 12345);
        assert_eq!(pos.format(), "mysql-bin.000001:12345");

        let parsed = BinlogPosition::parse("mysql-bin.000002:67890").unwrap();
        assert_eq!(parsed.filename, "mysql-bin.000002");
        assert_eq!(parsed.position, 67890);

        assert!(BinlogPosition::parse("invalid").is_none());
    }

    #[test]
    fn test_get_table() {
        let config = MySqlConfig::new("mysql://localhost/db")
            .with_table(MySqlTableConfig::new("db1", "users"))
            .with_table(MySqlTableConfig::new("db1", "orders"));

        assert!(config.get_table("db1", "users").is_some());
        assert!(config.get_table("db1", "orders").is_some());
        assert!(config.get_table("db1", "products").is_none());
    }

    #[test]
    fn test_config_serialization() {
        let config = MySqlConfig::new("mysql://localhost/db")
            .with_server_id(100)
            .with_table(MySqlTableConfig::new("test", "table1"));

        let json = serde_json::to_string(&config).unwrap();
        let parsed: MySqlConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.connection_url, config.connection_url);
        assert_eq!(parsed.server_id, config.server_id);
    }

    #[test]
    fn test_gtid_modes() {
        let modes = vec![GtidMode::Auto, GtidMode::On, GtidMode::Off];
        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: GtidMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn test_ssl_modes() {
        let modes = vec![
            SslMode::Disabled,
            SslMode::Preferred,
            SslMode::Required,
            SslMode::VerifyCa,
            SslMode::VerifyIdentity,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: SslMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
    }
}
