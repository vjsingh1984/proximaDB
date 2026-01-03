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

//! Unified Change Event Format
//!
//! This module defines the standard change event format used across all CDC
//! sources and sinks, providing a consistent interface regardless of the
//! underlying database system.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Unique identifier for change events
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(String);

impl EventId {
    /// Create a new unique event ID
    pub fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let random: u64 = rand::random();
        Self(format!("evt_{}_{:x}", timestamp, random))
    }

    /// Create from string
    pub fn from_string(s: String) -> Self {
        Self(s)
    }

    /// Get as string reference
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unified change event format for all CDC sources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEvent {
    /// Unique event ID
    pub id: EventId,
    /// Source system information
    pub source: SourceInfo,
    /// Event timestamp (epoch millis)
    pub timestamp: u64,
    /// Logical sequence number
    pub lsn: u64,
    /// Operation type
    pub operation: Operation,
    /// Collection/table name
    pub collection: String,
    /// Primary key/ID
    pub key: String,
    /// Before state (for updates/deletes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<RecordState>,
    /// After state (for inserts/updates)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<RecordState>,
    /// Transaction context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TransactionInfo>,
    /// Custom headers/properties
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

impl ChangeEvent {
    /// Create a new change event with basic info
    pub fn new(
        source: SourceInfo,
        operation: Operation,
        collection: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self {
            id: EventId::new(),
            source,
            timestamp: current_timestamp(),
            lsn: 0,
            operation,
            collection: collection.into(),
            key: key.into(),
            before: None,
            after: None,
            transaction: None,
            headers: HashMap::new(),
        }
    }

    /// Create a new insert event
    pub fn new_insert(
        source: SourceInfo,
        collection: String,
        key: String,
        after: RecordState,
    ) -> Self {
        Self {
            id: EventId::new(),
            source,
            timestamp: current_timestamp(),
            lsn: 0,
            operation: Operation::Insert,
            collection,
            key,
            before: None,
            after: Some(after),
            transaction: None,
            headers: HashMap::new(),
        }
    }

    /// Create a new update event
    pub fn new_update(
        source: SourceInfo,
        collection: String,
        key: String,
        before: RecordState,
        after: RecordState,
    ) -> Self {
        Self {
            id: EventId::new(),
            source,
            timestamp: current_timestamp(),
            lsn: 0,
            operation: Operation::Update,
            collection,
            key,
            before: Some(before),
            after: Some(after),
            transaction: None,
            headers: HashMap::new(),
        }
    }

    /// Create a new delete event
    pub fn new_delete(
        source: SourceInfo,
        collection: String,
        key: String,
        before: RecordState,
    ) -> Self {
        Self {
            id: EventId::new(),
            source,
            timestamp: current_timestamp(),
            lsn: 0,
            operation: Operation::Delete,
            collection,
            key,
            before: Some(before),
            after: None,
            transaction: None,
            headers: HashMap::new(),
        }
    }

    /// Create a snapshot event (initial load)
    pub fn new_snapshot(
        source: SourceInfo,
        collection: String,
        key: String,
        state: RecordState,
    ) -> Self {
        Self {
            id: EventId::new(),
            source,
            timestamp: current_timestamp(),
            lsn: 0,
            operation: Operation::Snapshot,
            collection,
            key,
            before: None,
            after: Some(state),
            transaction: None,
            headers: HashMap::new(),
        }
    }

    /// Set the LSN
    pub fn with_lsn(mut self, lsn: u64) -> Self {
        self.lsn = lsn;
        self
    }

    /// Set transaction info
    pub fn with_transaction(mut self, tx: TransactionInfo) -> Self {
        self.transaction = Some(tx);
        self
    }

    /// Add a header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Check if this is an insert operation
    pub fn is_insert(&self) -> bool {
        self.operation == Operation::Insert
    }

    /// Check if this is an update operation
    pub fn is_update(&self) -> bool {
        self.operation == Operation::Update
    }

    /// Check if this is a delete operation
    pub fn is_delete(&self) -> bool {
        self.operation == Operation::Delete
    }

    /// Get the vector from the after state (for inserts/updates)
    pub fn get_vector(&self) -> Option<&Vec<f32>> {
        self.after.as_ref().and_then(|s| s.vector.as_ref())
    }

    /// Get metadata from the after state
    pub fn get_metadata(&self) -> Option<&HashMap<String, serde_json::Value>> {
        self.after.as_ref().map(|s| &s.metadata)
    }

    /// Serialize to JSON bytes
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from JSON bytes
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Source system information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    /// Connector type
    pub connector: ConnectorType,
    /// Source database name
    pub database: String,
    /// Schema/namespace
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Server identifier
    pub server_id: String,
    /// Source version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl SourceInfo {
    /// Create a new source info with minimal parameters
    pub fn new(name: impl Into<String>, connector: ConnectorType) -> Self {
        Self {
            connector,
            database: name.into(),
            schema: None,
            server_id: "default".to_string(),
            version: None,
        }
    }

    /// Get the source name (database name)
    pub fn name(&self) -> &str {
        &self.database
    }

    /// Create PostgreSQL source info
    pub fn postgres(database: &str, schema: &str, server_id: &str) -> Self {
        Self {
            connector: ConnectorType::PostgreSQL,
            database: database.to_string(),
            schema: Some(schema.to_string()),
            server_id: server_id.to_string(),
            version: None,
        }
    }

    /// Create MySQL source info
    pub fn mysql(database: &str, server_id: &str) -> Self {
        Self {
            connector: ConnectorType::MySQL,
            database: database.to_string(),
            schema: None,
            server_id: server_id.to_string(),
            version: None,
        }
    }

    /// Create MongoDB source info
    pub fn mongodb(database: &str, server_id: &str) -> Self {
        Self {
            connector: ConnectorType::MongoDB,
            database: database.to_string(),
            schema: None,
            server_id: server_id.to_string(),
            version: None,
        }
    }

    /// Create ProximaDB source info (for outbound CDC)
    pub fn proximadb(database: &str, server_id: &str) -> Self {
        Self {
            connector: ConnectorType::ProximaDB,
            database: database.to_string(),
            schema: None,
            server_id: server_id.to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }
    }

    /// Set version
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
}

/// Connector type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectorType {
    /// PostgreSQL logical replication
    PostgreSQL,
    /// MySQL binlog replication
    MySQL,
    /// MongoDB change streams
    MongoDB,
    /// ProximaDB WAL (outbound CDC)
    ProximaDB,
    /// Generic/custom connector
    Custom,
}

impl std::fmt::Display for ConnectorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PostgreSQL => write!(f, "postgresql"),
            Self::MySQL => write!(f, "mysql"),
            Self::MongoDB => write!(f, "mongodb"),
            Self::ProximaDB => write!(f, "proximadb"),
            Self::Custom => write!(f, "custom"),
        }
    }
}

/// Operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    /// Row inserted
    Insert,
    /// Row updated
    Update,
    /// Row deleted
    Delete,
    /// Table truncated
    Truncate,
    /// Schema changed (DDL)
    SchemaChange,
    /// Initial snapshot
    Snapshot,
    /// Transaction begin
    Begin,
    /// Transaction commit
    Commit,
    /// Transaction rollback
    Rollback,
}

impl Operation {
    /// Check if operation affects data
    pub fn is_data_change(&self) -> bool {
        matches!(
            self,
            Operation::Insert | Operation::Update | Operation::Delete | Operation::Snapshot
        )
    }

    /// Check if operation is transactional marker
    pub fn is_transaction_marker(&self) -> bool {
        matches!(
            self,
            Operation::Begin | Operation::Commit | Operation::Rollback
        )
    }
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Insert => write!(f, "insert"),
            Self::Update => write!(f, "update"),
            Self::Delete => write!(f, "delete"),
            Self::Truncate => write!(f, "truncate"),
            Self::SchemaChange => write!(f, "schema_change"),
            Self::Snapshot => write!(f, "snapshot"),
            Self::Begin => write!(f, "begin"),
            Self::Commit => write!(f, "commit"),
            Self::Rollback => write!(f, "rollback"),
        }
    }
}

/// Record state (before or after)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordState {
    /// Vector data (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
    /// Metadata fields
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Raw source data (for debugging/replay)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

impl RecordState {
    /// Create empty state
    pub fn empty() -> Self {
        Self {
            vector: None,
            metadata: HashMap::new(),
            raw: None,
        }
    }

    /// Create state with vector
    pub fn with_vector(vector: Vec<f32>) -> Self {
        Self {
            vector: Some(vector),
            metadata: HashMap::new(),
            raw: None,
        }
    }

    /// Create state with metadata
    pub fn with_metadata(metadata: HashMap<String, serde_json::Value>) -> Self {
        Self {
            vector: None,
            metadata,
            raw: None,
        }
    }

    /// Create state from raw JSON
    pub fn from_raw(raw: serde_json::Value) -> Self {
        Self {
            vector: None,
            metadata: HashMap::new(),
            raw: Some(raw),
        }
    }

    /// Add vector
    pub fn set_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }

    /// Add metadata field
    pub fn add_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Set raw data
    pub fn set_raw(mut self, raw: serde_json::Value) -> Self {
        self.raw = Some(raw);
        self
    }

    /// Get metadata value
    pub fn get_metadata(&self, key: &str) -> Option<&serde_json::Value> {
        self.metadata.get(key)
    }

    /// Check if state has vector
    pub fn has_vector(&self) -> bool {
        self.vector.is_some()
    }
}

impl Default for RecordState {
    fn default() -> Self {
        Self::empty()
    }
}

/// Transaction information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionInfo {
    /// Transaction ID
    pub id: String,
    /// Begin timestamp
    pub begin_time: u64,
    /// Commit timestamp (if committed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_time: Option<u64>,
    /// Total events in transaction
    pub total_events: u32,
    /// Position of this event in transaction
    pub event_position: u32,
}

impl TransactionInfo {
    /// Create new transaction info
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            begin_time: current_timestamp(),
            commit_time: None,
            total_events: 0,
            event_position: 0,
        }
    }

    /// Set total events
    pub fn with_total_events(mut self, total: u32) -> Self {
        self.total_events = total;
        self
    }

    /// Set event position
    pub fn with_position(mut self, position: u32) -> Self {
        self.event_position = position;
        self
    }

    /// Mark as committed
    pub fn commit(mut self) -> Self {
        self.commit_time = Some(current_timestamp());
        self
    }

    /// Check if transaction is committed
    pub fn is_committed(&self) -> bool {
        self.commit_time.is_some()
    }

    /// Check if this is the last event in transaction
    pub fn is_last_event(&self) -> bool {
        self.event_position == self.total_events.saturating_sub(1)
    }
}

/// Get current timestamp in milliseconds
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_source() -> SourceInfo {
        SourceInfo::postgres("testdb", "public", "server1")
    }

    #[test]
    fn test_event_id_unique() {
        let id1 = EventId::new();
        let id2 = EventId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_new_insert_event() {
        let source = test_source();
        let state = RecordState::with_vector(vec![0.1, 0.2, 0.3]);

        let event =
            ChangeEvent::new_insert(source, "users".to_string(), "user_1".to_string(), state);

        assert!(event.is_insert());
        assert!(!event.is_update());
        assert!(!event.is_delete());
        assert!(event.before.is_none());
        assert!(event.after.is_some());
        assert_eq!(event.collection, "users");
        assert_eq!(event.key, "user_1");
    }

    #[test]
    fn test_new_update_event() {
        let source = test_source();
        let before = RecordState::with_vector(vec![0.1, 0.2]);
        let after = RecordState::with_vector(vec![0.3, 0.4]);

        let event = ChangeEvent::new_update(
            source,
            "products".to_string(),
            "prod_1".to_string(),
            before,
            after,
        );

        assert!(event.is_update());
        assert!(event.before.is_some());
        assert!(event.after.is_some());
    }

    #[test]
    fn test_new_delete_event() {
        let source = test_source();
        let before = RecordState::with_vector(vec![0.1, 0.2]);

        let event =
            ChangeEvent::new_delete(source, "items".to_string(), "item_1".to_string(), before);

        assert!(event.is_delete());
        assert!(event.before.is_some());
        assert!(event.after.is_none());
    }

    #[test]
    fn test_event_with_lsn() {
        let source = test_source();
        let state = RecordState::empty();

        let event = ChangeEvent::new_insert(source, "test".to_string(), "1".to_string(), state)
            .with_lsn(12345);

        assert_eq!(event.lsn, 12345);
    }

    #[test]
    fn test_event_with_transaction() {
        let source = test_source();
        let state = RecordState::empty();
        let tx = TransactionInfo::new("tx_123")
            .with_total_events(5)
            .with_position(2);

        let event = ChangeEvent::new_insert(source, "test".to_string(), "1".to_string(), state)
            .with_transaction(tx);

        assert!(event.transaction.is_some());
        let tx = event.transaction.unwrap();
        assert_eq!(tx.id, "tx_123");
        assert_eq!(tx.total_events, 5);
        assert_eq!(tx.event_position, 2);
    }

    #[test]
    fn test_source_info_constructors() {
        let pg = SourceInfo::postgres("db", "public", "srv1");
        assert_eq!(pg.connector, ConnectorType::PostgreSQL);
        assert_eq!(pg.schema, Some("public".to_string()));

        let mysql = SourceInfo::mysql("db", "srv2");
        assert_eq!(mysql.connector, ConnectorType::MySQL);
        assert!(mysql.schema.is_none());

        let mongo = SourceInfo::mongodb("db", "srv3");
        assert_eq!(mongo.connector, ConnectorType::MongoDB);

        let proxima = SourceInfo::proximadb("db", "srv4");
        assert_eq!(proxima.connector, ConnectorType::ProximaDB);
        assert!(proxima.version.is_some());
    }

    #[test]
    fn test_record_state_builder() {
        let state = RecordState::empty()
            .set_vector(vec![1.0, 2.0, 3.0])
            .add_metadata("category", serde_json::json!("electronics"))
            .add_metadata("price", serde_json::json!(99.99));

        assert!(state.has_vector());
        assert_eq!(state.vector.as_ref().unwrap().len(), 3);
        assert_eq!(
            state.get_metadata("category"),
            Some(&serde_json::json!("electronics"))
        );
    }

    #[test]
    fn test_operation_helpers() {
        assert!(Operation::Insert.is_data_change());
        assert!(Operation::Update.is_data_change());
        assert!(Operation::Delete.is_data_change());
        assert!(!Operation::Begin.is_data_change());

        assert!(!Operation::Insert.is_transaction_marker());
        assert!(Operation::Begin.is_transaction_marker());
        assert!(Operation::Commit.is_transaction_marker());
    }

    #[test]
    fn test_transaction_info() {
        let tx = TransactionInfo::new("tx_1")
            .with_total_events(10)
            .with_position(5);

        assert_eq!(tx.id, "tx_1");
        assert!(!tx.is_committed());
        assert!(!tx.is_last_event());

        let tx = tx.with_position(9);
        assert!(tx.is_last_event());

        let tx = tx.commit();
        assert!(tx.is_committed());
    }

    #[test]
    fn test_event_serialization() {
        let source = test_source();
        let state = RecordState::with_vector(vec![0.1, 0.2])
            .add_metadata("name", serde_json::json!("test"));

        let event = ChangeEvent::new_insert(source, "users".to_string(), "u1".to_string(), state);

        let bytes = event.to_json_bytes().unwrap();
        let parsed = ChangeEvent::from_json_bytes(&bytes).unwrap();

        assert_eq!(parsed.collection, "users");
        assert_eq!(parsed.key, "u1");
        assert!(parsed.is_insert());
    }

    #[test]
    fn test_get_vector() {
        let source = test_source();
        let state = RecordState::with_vector(vec![1.0, 2.0]);

        let event = ChangeEvent::new_insert(source, "test".to_string(), "1".to_string(), state);

        assert_eq!(event.get_vector(), Some(&vec![1.0, 2.0]));
    }

    #[test]
    fn test_connector_type_display() {
        assert_eq!(format!("{}", ConnectorType::PostgreSQL), "postgresql");
        assert_eq!(format!("{}", ConnectorType::MySQL), "mysql");
        assert_eq!(format!("{}", ConnectorType::MongoDB), "mongodb");
        assert_eq!(format!("{}", ConnectorType::ProximaDB), "proximadb");
    }
}
