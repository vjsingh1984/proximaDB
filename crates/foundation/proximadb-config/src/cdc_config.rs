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

//! CDC configuration types
//!
//! This module defines configuration for CDC sources, sinks, and transforms.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Main CDC configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CdcConfig {
    /// Offset storage configuration
    pub offset_storage: OffsetStorageConfig,
    /// Source configurations
    pub sources: Vec<SourceConfig>,
    /// Sink configurations
    pub sinks: Vec<SinkConfig>,
    /// Transform pipeline configuration
    pub transforms: Vec<TransformConfig>,
    /// Global settings
    pub settings: CdcSettings,
}

impl CdcConfig {
    /// Create a new CDC configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a source configuration
    pub fn with_source(mut self, source: SourceConfig) -> Self {
        self.sources.push(source);
        self
    }

    /// Add a sink configuration
    pub fn with_sink(mut self, sink: SinkConfig) -> Self {
        self.sinks.push(sink);
        self
    }

    /// Add a transform configuration
    pub fn with_transform(mut self, transform: TransformConfig) -> Self {
        self.transforms.push(transform);
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.sources.is_empty() {
            return Err("At least one source must be configured".to_string());
        }
        if self.sinks.is_empty() {
            return Err("At least one sink must be configured".to_string());
        }
        Ok(())
    }
}

/// Offset storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffsetStorageConfig {
    /// Storage type (file, memory, rocksdb)
    pub storage_type: OffsetStorageType,
    /// Base path for file-based storage
    pub path: Option<PathBuf>,
    /// Flush interval for batched writes
    pub flush_interval: Duration,
}

impl Default for OffsetStorageConfig {
    fn default() -> Self {
        Self {
            storage_type: OffsetStorageType::File,
            path: Some(PathBuf::from("/tmp/proximadb/cdc/offsets")),
            flush_interval: Duration::from_secs(5),
        }
    }
}

/// Offset storage type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OffsetStorageType {
    /// In-memory storage (lost on restart)
    Memory,
    /// File-based JSON storage
    File,
    /// RocksDB storage (high throughput)
    RocksDb,
}

/// Source connector configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    /// Unique name for this source
    pub name: String,
    /// Source type
    pub source_type: SourceType,
    /// Connection settings
    pub connection: ConnectionConfig,
    /// Tables/collections to capture
    pub capture: CaptureConfig,
    /// Snapshot configuration
    pub snapshot: SnapshotConfig,
    /// Source-specific settings
    pub settings: HashMap<String, String>,
}

impl SourceConfig {
    /// Create a PostgreSQL source configuration
    pub fn postgres(name: impl Into<String>, connection_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source_type: SourceType::PostgreSQL,
            connection: ConnectionConfig {
                url: connection_url.into(),
                ..Default::default()
            },
            capture: CaptureConfig::default(),
            snapshot: SnapshotConfig::default(),
            settings: HashMap::new(),
        }
    }

    /// Create a MySQL source configuration
    pub fn mysql(name: impl Into<String>, connection_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source_type: SourceType::MySQL,
            connection: ConnectionConfig {
                url: connection_url.into(),
                ..Default::default()
            },
            capture: CaptureConfig::default(),
            snapshot: SnapshotConfig::default(),
            settings: HashMap::new(),
        }
    }

    /// Create a MongoDB source configuration
    pub fn mongodb(name: impl Into<String>, connection_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source_type: SourceType::MongoDB,
            connection: ConnectionConfig {
                url: connection_url.into(),
                ..Default::default()
            },
            capture: CaptureConfig::default(),
            snapshot: SnapshotConfig::default(),
            settings: HashMap::new(),
        }
    }

    /// Create a ProximaDB source configuration
    pub fn proximadb(name: impl Into<String>, connection_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source_type: SourceType::ProximaDB,
            connection: ConnectionConfig {
                url: connection_url.into(),
                ..Default::default()
            },
            capture: CaptureConfig::default(),
            snapshot: SnapshotConfig::default(),
            settings: HashMap::new(),
        }
    }

    /// Set tables to capture
    pub fn with_tables(mut self, tables: Vec<String>) -> Self {
        self.capture.include_tables = tables;
        self
    }

    /// Add a setting
    pub fn with_setting(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.settings.insert(key.into(), value.into());
        self
    }
}

/// Source database type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    /// PostgreSQL logical replication
    PostgreSQL,
    /// MySQL binlog
    MySQL,
    /// MongoDB change streams
    MongoDB,
    /// ProximaDB WAL
    ProximaDB,
}

/// Connection configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionConfig {
    /// Connection URL
    pub url: String,
    /// Username (if not in URL)
    pub username: Option<String>,
    /// Password (if not in URL)
    pub password: Option<String>,
    /// SSL mode
    pub ssl_mode: SslMode,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Max retries on connection failure
    pub max_retries: u32,
    /// Retry delay
    pub retry_delay: Duration,
}

impl ConnectionConfig {
    /// Create a new connection configuration
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Default::default()
        }
    }
}

/// SSL mode for connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SslMode {
    /// No SSL
    #[default]
    Disable,
    /// Prefer SSL but allow unencrypted
    Prefer,
    /// Require SSL
    Require,
    /// Verify server certificate
    VerifyCa,
    /// Verify server certificate and hostname
    VerifyFull,
}

/// Capture configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// Tables to include (empty = all)
    pub include_tables: Vec<String>,
    /// Tables to exclude
    pub exclude_tables: Vec<String>,
    /// Columns to include (per table)
    pub include_columns: HashMap<String, Vec<String>>,
    /// Columns to exclude (per table)
    pub exclude_columns: HashMap<String, Vec<String>>,
    /// Operations to capture
    pub operations: Vec<CaptureOperation>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            include_tables: Vec::new(),
            exclude_tables: Vec::new(),
            include_columns: HashMap::new(),
            exclude_columns: HashMap::new(),
            operations: vec![
                CaptureOperation::Insert,
                CaptureOperation::Update,
                CaptureOperation::Delete,
            ],
        }
    }
}

/// Operations to capture
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureOperation {
    /// Insert operations
    Insert,
    /// Update operations
    Update,
    /// Delete operations
    Delete,
    /// Truncate operations
    Truncate,
}

/// Snapshot configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotConfig {
    /// Whether to take initial snapshot
    pub enabled: bool,
    /// Snapshot mode
    pub mode: SnapshotMode,
    /// Batch size for snapshot reads
    pub batch_size: usize,
    /// Tables to snapshot (empty = use capture config)
    pub tables: Vec<String>,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: SnapshotMode::Initial,
            batch_size: 10_000,
            tables: Vec::new(),
        }
    }
}

/// Snapshot mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    /// Take snapshot only if no offset exists
    Initial,
    /// Always take a new snapshot
    Always,
    /// Never take a snapshot
    Never,
    /// Export snapshot to file
    ExportToFile,
}

/// Sink configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SinkConfig {
    /// Unique name for this sink
    pub name: String,
    /// Sink type
    pub sink_type: SinkType,
    /// Connection settings
    pub connection: ConnectionConfig,
    /// Batching configuration
    pub batching: BatchConfig,
    /// Delivery guarantees
    pub delivery: DeliveryConfig,
    /// Sink-specific settings
    pub settings: HashMap<String, String>,
}

impl SinkConfig {
    /// Create a Kafka sink configuration
    pub fn kafka(name: impl Into<String>, brokers: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sink_type: SinkType::Kafka,
            connection: ConnectionConfig::new(brokers),
            batching: BatchConfig::default(),
            delivery: DeliveryConfig::default(),
            settings: HashMap::new(),
        }
    }

    /// Create a webhook sink configuration
    pub fn webhook(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sink_type: SinkType::Webhook,
            connection: ConnectionConfig::new(url),
            batching: BatchConfig::default(),
            delivery: DeliveryConfig::default(),
            settings: HashMap::new(),
        }
    }

    /// Create a ProximaDB sink configuration
    pub fn proximadb(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sink_type: SinkType::ProximaDB,
            connection: ConnectionConfig::new(url),
            batching: BatchConfig::default(),
            delivery: DeliveryConfig::default(),
            settings: HashMap::new(),
        }
    }

    /// Add a setting
    pub fn with_setting(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.settings.insert(key.into(), value.into());
        self
    }
}

/// Sink type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SinkType {
    /// Apache Kafka
    Kafka,
    /// HTTP webhook
    Webhook,
    /// ProximaDB (for vectorization)
    ProximaDB,
    /// Amazon S3
    S3,
    /// Google Cloud Storage
    Gcs,
    /// Azure Blob Storage
    Azure,
}

/// Batching configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Maximum batch size
    pub max_size: usize,
    /// Maximum batch timeout
    pub max_timeout: Duration,
    /// Maximum bytes per batch
    pub max_bytes: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_size: 1000,
            max_timeout: Duration::from_secs(1),
            max_bytes: 1024 * 1024, // 1MB
        }
    }
}

/// Delivery guarantees configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryConfig {
    /// Delivery guarantee level
    pub guarantee: DeliveryGuarantee,
    /// Maximum retries
    pub max_retries: u32,
    /// Retry backoff
    pub retry_backoff: Duration,
    /// Maximum retry backoff
    pub max_retry_backoff: Duration,
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            guarantee: DeliveryGuarantee::AtLeastOnce,
            max_retries: 3,
            retry_backoff: Duration::from_millis(100),
            max_retry_backoff: Duration::from_secs(30),
        }
    }
}

/// Delivery guarantee level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryGuarantee {
    /// Fire and forget
    AtMostOnce,
    /// Retry until successful
    AtLeastOnce,
    /// Exactly once (requires transactional support)
    ExactlyOnce,
}

/// Transform pipeline configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformConfig {
    /// Transform name
    pub name: String,
    /// Transform type
    pub transform_type: TransformType,
    /// Source events to transform (regex pattern)
    pub source_pattern: Option<String>,
    /// Transform-specific settings
    pub settings: HashMap<String, String>,
}

impl TransformConfig {
    /// Create a schema mapping transform
    pub fn schema_mapping(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            transform_type: TransformType::SchemaMapping,
            source_pattern: None,
            settings: HashMap::new(),
        }
    }

    /// Create a vectorization transform
    pub fn vectorization(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            transform_type: TransformType::Vectorization,
            source_pattern: None,
            settings: HashMap::new(),
        }
    }

    /// Set source pattern
    pub fn with_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.source_pattern = Some(pattern.into());
        self
    }

    /// Add a setting
    pub fn with_setting(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.settings.insert(key.into(), value.into());
        self
    }
}

/// Transform type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformType {
    /// Map source schema to target
    SchemaMapping,
    /// Convert text to vectors
    Vectorization,
    /// Filter events
    Filter,
    /// Route events to different sinks
    Router,
    /// Custom script transform
    Script,
}

/// Global CDC settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdcSettings {
    /// Worker thread count
    pub worker_threads: usize,
    /// Queue size for events
    pub queue_size: usize,
    /// Metrics collection interval
    pub metrics_interval: Duration,
    /// Health check interval
    pub health_check_interval: Duration,
    /// Enable event deduplication
    pub deduplication: bool,
    /// Deduplication window
    pub dedup_window: Duration,
}

impl Default for CdcSettings {
    fn default() -> Self {
        Self {
            worker_threads: 4,
            queue_size: 10_000,
            metrics_interval: Duration::from_secs(10),
            health_check_interval: Duration::from_secs(30),
            deduplication: true,
            dedup_window: Duration::from_secs(60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CdcConfig::default();
        assert!(config.sources.is_empty());
        assert!(config.sinks.is_empty());
    }

    #[test]
    fn test_config_builder() {
        let config = CdcConfig::new()
            .with_source(SourceConfig::postgres(
                "pg_source",
                "postgres://localhost/mydb",
            ))
            .with_sink(SinkConfig::kafka("kafka_sink", "localhost:9092"))
            .with_transform(TransformConfig::vectorization("text_to_vec"));

        assert_eq!(config.sources.len(), 1);
        assert_eq!(config.sinks.len(), 1);
        assert_eq!(config.transforms.len(), 1);
    }

    #[test]
    fn test_source_config() {
        let source = SourceConfig::postgres("pg", "postgres://localhost/db")
            .with_tables(vec!["users".to_string(), "orders".to_string()])
            .with_setting("slot.name", "cdc_slot");

        assert_eq!(source.source_type, SourceType::PostgreSQL);
        assert_eq!(source.capture.include_tables.len(), 2);
        assert!(source.settings.contains_key("slot.name"));
    }

    #[test]
    fn test_sink_config() {
        let sink = SinkConfig::kafka("kafka", "localhost:9092").with_setting("topic", "cdc_events");

        assert_eq!(sink.sink_type, SinkType::Kafka);
        assert!(sink.settings.contains_key("topic"));
    }

    #[test]
    fn test_transform_config() {
        let transform = TransformConfig::schema_mapping("map1")
            .with_pattern("pg_source.*")
            .with_setting("target_collection", "vectors");

        assert_eq!(transform.transform_type, TransformType::SchemaMapping);
        assert!(transform.source_pattern.is_some());
    }

    #[test]
    fn test_config_validation() {
        let config = CdcConfig::default();
        assert!(config.validate().is_err());

        let config = CdcConfig::new()
            .with_source(SourceConfig::postgres("pg", "postgres://localhost/db"))
            .with_sink(SinkConfig::kafka("kafka", "localhost:9092"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_serialization() {
        let config = CdcConfig::new()
            .with_source(SourceConfig::postgres("pg", "postgres://localhost/db"))
            .with_sink(SinkConfig::webhook("hook", "https://example.com/webhook"));

        let json = serde_json::to_string(&config).unwrap();
        let parsed: CdcConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.sinks.len(), 1);
    }

    #[test]
    fn test_source_types() {
        let pg = SourceConfig::postgres("pg", "postgres://localhost/db");
        let mysql = SourceConfig::mysql("mysql", "mysql://localhost/db");
        let mongo = SourceConfig::mongodb("mongo", "mongodb://localhost/db");
        let proxima = SourceConfig::proximadb("proxima", "http://localhost:5678");

        assert_eq!(pg.source_type, SourceType::PostgreSQL);
        assert_eq!(mysql.source_type, SourceType::MySQL);
        assert_eq!(mongo.source_type, SourceType::MongoDB);
        assert_eq!(proxima.source_type, SourceType::ProximaDB);
    }

    #[test]
    fn test_capture_config() {
        let capture = CaptureConfig::default();
        assert_eq!(capture.operations.len(), 3);
        assert!(capture.operations.contains(&CaptureOperation::Insert));
        assert!(capture.operations.contains(&CaptureOperation::Update));
        assert!(capture.operations.contains(&CaptureOperation::Delete));
    }
}
