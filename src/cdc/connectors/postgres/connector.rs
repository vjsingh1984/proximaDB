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

//! PostgreSQL CDC connector implementation
//!
//! This connector uses PostgreSQL's logical replication with the pgoutput
//! protocol to capture changes from a PostgreSQL database.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, mpsc};
use tracing::info;

use super::config::PostgresConfig;
use super::decoder::{ColumnValue, PgOutputDecoder, PgOutputEvent, TupleData};
use super::replication::ReplicationStream;
use crate::cdc::config::SourceConfig;
use crate::cdc::error::{CdcError, CdcResult};
use crate::cdc::event::{ChangeEvent, Operation, RecordState, SourceInfo, TransactionInfo};
use crate::cdc::offset::{Offset, OffsetStore};
use crate::cdc::source::{BaseSource, CdcSource, SourceHandle, SourceStatus};

/// PostgreSQL CDC connector
pub struct PostgresConnector {
    /// Base source functionality
    base: BaseSource,
    /// PostgreSQL-specific configuration
    pg_config: PostgresConfig,
    /// Offset store
    offset_store: Arc<dyn OffsetStore>,
    /// Protocol decoder
    decoder: Arc<RwLock<PgOutputDecoder>>,
    /// Current LSN
    current_lsn: Arc<AtomicU64>,
    /// Current transaction
    current_tx: Arc<RwLock<Option<TransactionInfo>>>,
    /// Replication stream connection
    replication_stream: Arc<RwLock<Option<ReplicationStream>>>,
}

impl PostgresConnector {
    /// Create a new PostgreSQL connector
    pub async fn new(
        pg_config: PostgresConfig,
        offset_store: Arc<dyn OffsetStore>,
    ) -> CdcResult<Self> {
        pg_config.validate().map_err(CdcError::Configuration)?;

        // Create source config from pg_config
        let source_config =
            SourceConfig::postgres(&pg_config.slot_name, &pg_config.connection_string);

        Ok(Self {
            base: BaseSource::new(source_config),
            pg_config,
            offset_store,
            decoder: Arc::new(RwLock::new(PgOutputDecoder::new())),
            current_lsn: Arc::new(AtomicU64::new(0)),
            current_tx: Arc::new(RwLock::new(None)),
            replication_stream: Arc::new(RwLock::new(None)),
        })
    }

    /// Get PostgreSQL configuration
    pub fn pg_config(&self) -> &PostgresConfig {
        &self.pg_config
    }

    /// Convert pgoutput event to ChangeEvent
    async fn to_change_event(&self, event: PgOutputEvent, lsn: u64) -> Option<ChangeEvent> {
        match event {
            PgOutputEvent::Begin { xid, .. } => {
                let tx = TransactionInfo::new(format!("pg_tx_{}", xid));
                *self.current_tx.write().await = Some(tx);
                None
            }
            PgOutputEvent::Commit { xid: _, .. } => {
                let mut tx = self.current_tx.write().await.take()?;
                tx = tx.commit();
                // We could emit a transaction marker here if needed
                let _ = tx;
                None
            }
            PgOutputEvent::Insert {
                relation, tuple, ..
            } => {
                let relation = relation?;
                let after = self.tuple_to_record_state(&tuple)?;

                let key = self.extract_key(&tuple, &relation);

                let mut event = ChangeEvent::new(
                    self.create_source_info(&relation),
                    Operation::Insert,
                    relation.full_name(),
                    key,
                )
                .with_lsn(lsn);

                // Add after state
                if let Ok(e) = self.add_after_state(event, after) {
                    event = e;
                } else {
                    return None;
                }

                if let Some(tx) = self.current_tx.read().await.clone() {
                    event = event.with_transaction(tx);
                }

                Some(event)
            }
            PgOutputEvent::Update {
                relation,
                old_tuple,
                new_tuple,
                ..
            } => {
                let relation = relation?;
                let new_tuple = new_tuple?;

                let after = self.tuple_to_record_state(&new_tuple)?;
                let before = old_tuple.and_then(|t| self.tuple_to_record_state(&t));

                let key = self.extract_key(&new_tuple, &relation);

                let source = self.create_source_info(&relation);
                let mut event = if let Some(before) = before {
                    ChangeEvent::new_update(source, relation.full_name(), key, before, after)
                } else {
                    let mut e =
                        ChangeEvent::new(source, Operation::Update, relation.full_name(), key);
                    if let Ok(ev) = self.add_after_state(e, after) {
                        e = ev;
                    } else {
                        return None;
                    }
                    e
                };

                event = event.with_lsn(lsn);

                if let Some(tx) = self.current_tx.read().await.clone() {
                    event = event.with_transaction(tx);
                }

                Some(event)
            }
            PgOutputEvent::Delete {
                relation,
                key_tuple,
                ..
            } => {
                let relation = relation?;
                let before = self.tuple_to_record_state(&key_tuple)?;
                let key = self.extract_key(&key_tuple, &relation);

                let mut event = ChangeEvent::new_delete(
                    self.create_source_info(&relation),
                    relation.full_name(),
                    key,
                    before,
                )
                .with_lsn(lsn);

                if let Some(tx) = self.current_tx.read().await.clone() {
                    event = event.with_transaction(tx);
                }

                Some(event)
            }
            PgOutputEvent::Truncate { relation_ids, .. } => {
                // Emit truncate events for each relation
                // For now, we skip these as they're less common
                let _ = relation_ids;
                None
            }
            _ => None,
        }
    }

    /// Convert tuple to RecordState
    fn tuple_to_record_state(&self, tuple: &TupleData) -> Option<RecordState> {
        let mut metadata = HashMap::new();
        let mut vector = None;

        for (name, value) in &tuple.values {
            let Some(name) = name else { continue };

            // Check if this is the vector column
            if Some(name.as_str())
                == self
                    .pg_config
                    .tables
                    .first()
                    .and_then(|t| t.vector_column.as_deref())
            {
                if let Some(v) = self.parse_vector(value) {
                    vector = Some(v);
                }
                continue;
            }

            // Add to metadata
            let json_value = match value {
                ColumnValue::Null => serde_json::Value::Null,
                ColumnValue::Text(s) => {
                    // Try to parse as JSON, fall back to string
                    serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.clone()))
                }
                ColumnValue::Binary(b) => serde_json::Value::String(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    b,
                )),
                ColumnValue::Unchanged => continue,
            };

            metadata.insert(name.clone(), json_value);
        }

        Some(RecordState {
            vector,
            metadata,
            raw: None,
        })
    }

    /// Parse vector from column value
    fn parse_vector(&self, value: &ColumnValue) -> Option<Vec<f32>> {
        match value {
            ColumnValue::Text(s) => {
                // Try to parse as JSON array
                if let Ok(arr) = serde_json::from_str::<Vec<f32>>(s) {
                    return Some(arr);
                }
                // Try PostgreSQL array format: {1.0,2.0,3.0}
                if s.starts_with('{') && s.ends_with('}') {
                    let inner = &s[1..s.len() - 1];
                    let parsed: Result<Vec<f32>, _> =
                        inner.split(',').map(|n| n.trim().parse::<f32>()).collect();
                    return parsed.ok();
                }
                None
            }
            _ => None,
        }
    }

    /// Extract primary key from tuple
    fn extract_key(&self, tuple: &TupleData, relation: &super::decoder::PgRelation) -> String {
        // Find primary key columns
        let key_columns: Vec<&str> = relation
            .columns
            .iter()
            .filter(|c| c.is_key)
            .map(|c| c.name.as_str())
            .collect();

        if key_columns.is_empty() {
            // Use first column as fallback
            if let Some((_, value)) = tuple.values.first()
                && let ColumnValue::Text(s) = value
            {
                return s.clone();
            }
            return "unknown".to_string();
        }

        // Build composite key
        let key_parts: Vec<String> = key_columns
            .iter()
            .filter_map(|name| {
                tuple.get(name).and_then(|v| match v {
                    ColumnValue::Text(s) => Some(s.clone()),
                    _ => None,
                })
            })
            .collect();

        if key_parts.len() == 1 {
            key_parts.into_iter().next().unwrap_or_default()
        } else {
            key_parts.join(":")
        }
    }

    /// Create source info for a relation
    fn create_source_info(&self, relation: &super::decoder::PgRelation) -> SourceInfo {
        SourceInfo::postgres(
            &self.pg_config.slot_name,
            &relation.namespace,
            &self.pg_config.slot_name,
        )
    }

    /// Helper to add after state to an event
    fn add_after_state(
        &self,
        mut event: ChangeEvent,
        after: RecordState,
    ) -> CdcResult<ChangeEvent> {
        event.after = Some(after);
        Ok(event)
    }

    /// Get current LSN
    pub async fn current_lsn(&self) -> u64 {
        self.current_lsn.load(Ordering::Acquire)
    }

    /// Update current LSN
    pub async fn update_lsn(&self, lsn: u64) {
        self.current_lsn.store(lsn, Ordering::Release);
    }

    /// Process raw replication data
    pub async fn process_data(
        &self,
        data: &[u8],
        lsn: u64,
        event_tx: &mpsc::Sender<ChangeEvent>,
    ) -> CdcResult<usize> {
        let events = {
            let mut decoder = self.decoder.write().await;
            decoder.decode(data)?
        };

        let mut count = 0;
        for pg_event in events {
            if let Some(change_event) = self.to_change_event(pg_event, lsn).await {
                event_tx
                    .send(change_event)
                    .await
                    .map_err(|e| CdcError::Coordinator(format!("Failed to send event: {}", e)))?;
                count += 1;
            }
        }

        self.update_lsn(lsn).await;
        Ok(count)
    }
}

#[async_trait::async_trait]
impl CdcSource for PostgresConnector {
    fn name(&self) -> &str {
        &self.base.config().name
    }

    fn status(&self) -> SourceStatus {
        self.base.status()
    }

    async fn start(
        &mut self,
        _event_tx: mpsc::Sender<ChangeEvent>,
        offset_store: Arc<dyn OffsetStore>,
    ) -> CdcResult<SourceHandle> {
        let _shutdown_rx = self.base.init_shutdown();
        self.base.set_status(SourceStatus::Connecting);

        info!(
            "Starting PostgreSQL CDC connector for slot: {}",
            self.pg_config.slot_name
        );

        // Load last known offset
        let _start_lsn = if let Ok(Some(offset)) = offset_store.get(&self.pg_config.slot_name).await
        {
            self.current_lsn.store(offset.lsn, Ordering::Release);
            offset.lsn
        } else {
            0
        };

        // Create replication stream
        let mut stream = ReplicationStream::new(self.pg_config.clone());

        // Connect to PostgreSQL
        stream.connect().await.map_err(|e| {
            CdcError::Coordinator(format!("Failed to connect to PostgreSQL: {}", e))
        })?;

        // Store the stream
        *self.replication_stream.write().await = Some(stream);

        self.base.set_status(SourceStatus::Streaming);

        Ok(self
            .base
            .create_handle()
            .ok_or_else(|| CdcError::Coordinator("Failed to create source handle".to_string()))?)
    }

    async fn stop(&mut self) -> CdcResult<()> {
        self.base.set_status(SourceStatus::Stopping);

        // Save current offset
        let lsn = self.current_lsn.load(Ordering::Acquire);
        if lsn > 0 {
            let offset = Offset::new(&self.pg_config.slot_name, lsn);
            self.offset_store.store(&offset).await?;
        }

        self.base.set_status(SourceStatus::Stopped);
        Ok(())
    }

    async fn pause(&mut self) -> CdcResult<()> {
        self.base.set_status(SourceStatus::Paused);
        Ok(())
    }

    async fn resume(&mut self) -> CdcResult<()> {
        self.base.set_status(SourceStatus::Streaming);
        Ok(())
    }

    async fn current_offset(&self) -> CdcResult<Option<Offset>> {
        let lsn = self.current_lsn.load(Ordering::Acquire);
        if lsn == 0 {
            return Ok(None);
        }
        Ok(Some(Offset::new(&self.pg_config.slot_name, lsn)))
    }

    fn config(&self) -> &SourceConfig {
        self.base.config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::offset::MemoryOffsetStore;

    async fn create_test_connector() -> PostgresConnector {
        let pg_config = PostgresConfig::new("postgres://localhost/test")
            .with_slot("test_slot")
            .with_publication("test_pub");

        let offset_store = Arc::new(MemoryOffsetStore::new());
        PostgresConnector::new(pg_config, offset_store)
            .await
            .expect("Failed to create test connector")
    }

    #[tokio::test]
    async fn test_connector_creation() {
        let connector = create_test_connector().await;
        assert_eq!(connector.name(), "test_slot");
        assert_eq!(connector.status(), SourceStatus::Created);
    }

    #[tokio::test]
    async fn test_connector_config() {
        let connector = create_test_connector().await;
        assert_eq!(connector.pg_config().slot_name, "test_slot");
        assert_eq!(connector.pg_config().publication, "test_pub");
    }

    #[tokio::test]
    async fn test_parse_vector_json() {
        let connector = create_test_connector().await;

        let value = ColumnValue::Text("[1.0, 2.0, 3.0]".to_string());
        let result = connector.parse_vector(&value);
        assert!(result.is_some());
        assert_eq!(
            result.expect("Vector parsing should succeed"),
            vec![1.0, 2.0, 3.0]
        );
    }

    #[tokio::test]
    async fn test_parse_vector_postgres_array() {
        let connector = create_test_connector().await;

        let value = ColumnValue::Text("{1.5, 2.5, 3.5}".to_string());
        let result = connector.parse_vector(&value);
        assert!(result.is_some());
        assert_eq!(
            result.expect("Vector parsing should succeed"),
            vec![1.5, 2.5, 3.5]
        );
    }

    #[tokio::test]
    async fn test_lsn_tracking() {
        let connector = create_test_connector().await;

        assert_eq!(connector.current_lsn().await, 0);

        connector.update_lsn(12345).await;
        assert_eq!(connector.current_lsn().await, 12345);
    }

    #[cfg(feature = "experimental-cdc-connectors")]
    #[tokio::test]
    async fn test_connector_start_stop() {
        let mut connector = create_test_connector().await;
        let (tx, _rx) = mpsc::channel(10);
        let offset_store = Arc::new(MemoryOffsetStore::new());

        let handle = connector
            .start(tx, offset_store)
            .await
            .expect("Failed to start connector");
        assert_eq!(connector.status(), SourceStatus::Streaming);

        handle.stop();
        connector.stop().await.expect("Failed to stop connector");
        assert_eq!(connector.status(), SourceStatus::Stopped);
    }

    #[cfg(feature = "experimental-cdc-connectors")]
    #[tokio::test]
    async fn test_connector_pause_resume() {
        let mut connector = create_test_connector().await;
        let (tx, _rx) = mpsc::channel(10);
        let offset_store = Arc::new(MemoryOffsetStore::new());

        connector
            .start(tx, offset_store)
            .await
            .expect("Failed to start connector");

        connector.pause().await.expect("Failed to pause connector");
        assert_eq!(connector.status(), SourceStatus::Paused);

        connector
            .resume()
            .await
            .expect("Failed to resume connector");
        assert_eq!(connector.status(), SourceStatus::Streaming);
    }

    #[tokio::test]
    async fn test_current_offset() {
        let connector = create_test_connector().await;

        // Initially no offset
        let current_offset = connector
            .current_offset()
            .await
            .expect("Failed to get current offset");
        assert!(current_offset.is_none());

        // After updating LSN
        connector.update_lsn(54321).await;
        let offset = connector
            .current_offset()
            .await
            .expect("Failed to get current offset")
            .expect("Offset should exist after updating LSN");
        assert_eq!(offset.lsn, 54321);
    }
}
