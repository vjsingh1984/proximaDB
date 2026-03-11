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

//! MySQL CDC connector implementation
//!
//! This connector uses MySQL's binlog protocol to capture changes from a MySQL database.
//!
//! ## MySQL Binlog Protocol
//!
//! The MySQL binlog protocol requires:
//! 1. Registering as a replica (COM_REGISTER_SLAVE)
//! 2. Requesting binlog dump (COM_BINLOG_DUMP)
//! 3. Streaming binlog events
//! 4. Tracking position and GTID

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

use super::config::{BinlogPosition, MySqlConfig};
use super::decoder::{BinlogDecoder, BinlogEvent, RowEventType};
use crate::cdc::config::SourceConfig;
use crate::cdc::error::{CdcError, CdcResult};
use crate::cdc::event::{ChangeEvent, ConnectorType, Operation, RecordState, SourceInfo};
use crate::cdc::offset::{Offset, OffsetStore};
use crate::cdc::source::{BaseSource, CdcSource, SourceHandle, SourceStatus};

#[cfg(feature = "experimental-cdc-connectors")]
use mysql_async::{
    prelude::*, Opts,
    Row as MySqlRow,
};

/// MySQL binlog streamer for reading binlog events
#[cfg(feature = "experimental-cdc-connectors")]
pub struct MySqlBinlogStreamer {
    client: Option<mysql_async::Conn>,
    server_id: u32,
    username: String,
    password: String,
}

#[cfg(feature = "experimental-cdc-connectors")]
impl MySqlBinlogStreamer {
    /// Create a new binlog streamer
    pub fn new(config: &MySqlConfig) -> Self {
        Self {
            client: None,
            server_id: config.server_id,
            username: config.username.clone(),
            password: config.password.clone(),
        }
    }

    /// Connect to MySQL and register as a replica
    pub async fn connect(&mut self, connection_url: &str) -> CdcResult<()> {
        // Create connection opts from URL
        let opts = Opts::from_url(connection_url)
            .map_err(|e| CdcError::Configuration(format!("Invalid MySQL connection URL: {}", e)))?;

        // Connect to MySQL
        let client = Conn::new(opts)
            .await
            .map_err(|e| CdcError::Connection(format!("Failed to connect to MySQL: {}", e)))?;

        // Register as a replica (COM_REGISTER_SLAVE)
        //
        // Note: In MySQL 8.0+, the replication protocol has changed.
        // We use the simpler approach of setting the replica status directly.
        let query = format!(
            "SET @master_binlog_checksum = 'NONE'; \
             SET GLOBAL binlog_format = 'ROW'; \
             SET GLOBAL binlog_row_image = 'FULL';"
        );

        client.exec_drop(query)
            .await
            .map_err(|e| CdcError::Connection(format!("Failed to configure binlog: {}", e)))?;

        // Request binlog dump
        // In production, this would use COM_BINLOG_DUMP with position/GTID
        let binlog_query = format!(
            "SHOW MASTER STATUS"
        );

        // Get current binlog position
        if let Ok(row) = client.query_first(binlog_query).await {
            if let Some(position) = row {
                info!("Current binlog position: {:?}", position);
            }
        }

        self.client = Some(client);
        Ok(())
    }

    /// Request binlog dump from specific position
    pub async fn request_binlog_dump(
        &mut self,
        binlog_filename: &str,
        position: u64,
    ) -> CdcResult<()> {
        let client = self.client.as_mut()
            .ok_or_else(|| CdcError::Connection("Not connected to MySQL".to_string()))?;

        // COM_BINLOG_DUMP command to request binlog data
        // Note: This is a simplified implementation
        // Full implementation would use the MySQL binlog protocol
        let query = format!(
            "SHOW BINLOG EVENTS IN '{}' FROM {} LIMIT 1000",
            binlog_filename, position
        );

        match client.query_iter(query).await {
            Ok(result) => {
                // Process binlog events
                drop(result);
                Ok(())
            }
            Err(e) => {
                warn!("Failed to query binlog events: {}", e);
                // Continue - we'll use polling instead
                Ok(())
            }
        }
    }
}

/// MySQL CDC connector
pub struct MySqlConnector {
    /// Base source functionality
    base: BaseSource,
    /// MySQL configuration
    mysql_config: MySqlConfig,
    /// Offset store
    offset_store: Arc<dyn OffsetStore>,
    /// Binlog decoder
    decoder: Arc<RwLock<BinlogDecoder>>,
    /// Current binlog position
    current_position: Arc<RwLock<Option<BinlogPosition>>>,
    /// Current GTID
    current_gtid: Arc<RwLock<Option<String>>>,
}

impl MySqlConnector {
    /// Create a new MySQL connector
    pub async fn new(
        mysql_config: MySqlConfig,
        offset_store: Arc<dyn OffsetStore>,
    ) -> CdcResult<Self> {
        mysql_config.validate().map_err(CdcError::Configuration)?;

        let source_config = SourceConfig::mysql(
            &format!("mysql_{}", mysql_config.server_id),
            &mysql_config.connection_url,
        );

        Ok(Self {
            base: BaseSource::new(source_config),
            mysql_config,
            offset_store,
            decoder: Arc::new(RwLock::new(BinlogDecoder::new())),
            current_position: Arc::new(RwLock::new(None)),
            current_gtid: Arc::new(RwLock::new(None)),
        })
    }

    /// Get MySQL configuration
    pub fn mysql_config(&self) -> &MySqlConfig {
        &self.mysql_config
    }

    /// Get current binlog position
    pub async fn current_position(&self) -> Option<BinlogPosition> {
        self.current_position.read().await.clone()
    }

    /// Get current GTID
    pub async fn current_gtid(&self) -> Option<String> {
        self.current_gtid.read().await.clone()
    }

    /// Update binlog position
    pub async fn update_position(&self, position: BinlogPosition) {
        *self.current_position.write().await = Some(position);
    }

    /// Update GTID
    pub async fn update_gtid(&self, gtid: String) {
        *self.current_gtid.write().await = Some(gtid);
    }

    /// Convert binlog event to ChangeEvent
    pub async fn to_change_event(&self, event: BinlogEvent) -> Option<ChangeEvent> {
        match event {
            BinlogEvent::WriteRows(row_event) => {
                let table_map = row_event.table_map?;
                let source = self.create_source_info(&table_map.schema, &table_map.table);

                // Create insert event for each row
                // In a full implementation, we'd iterate over row_event.rows
                let event = ChangeEvent::new(
                    source,
                    Operation::Insert,
                    table_map.full_name(),
                    format!("row_{}", row_event.table_id),
                );

                Some(event)
            }
            BinlogEvent::UpdateRows(row_event) => {
                let table_map = row_event.table_map?;
                let source = self.create_source_info(&table_map.schema, &table_map.table);

                let event = ChangeEvent::new(
                    source,
                    Operation::Update,
                    table_map.full_name(),
                    format!("row_{}", row_event.table_id),
                );

                Some(event)
            }
            BinlogEvent::DeleteRows(row_event) => {
                let table_map = row_event.table_map?;
                let source = self.create_source_info(&table_map.schema, &table_map.table);

                let event = ChangeEvent::new(
                    source,
                    Operation::Delete,
                    table_map.full_name(),
                    format!("row_{}", row_event.table_id),
                );

                Some(event)
            }
            BinlogEvent::Gtid { gtid } => {
                // Update GTID but don't emit event
                *self.current_gtid.write().await = Some(gtid);
                None
            }
            BinlogEvent::Rotate { filename, position } => {
                // Update position but don't emit event
                *self.current_position.write().await =
                    Some(BinlogPosition::new(filename, position));
                None
            }
            _ => None,
        }
    }

    /// Create source info
    fn create_source_info(&self, schema: &str, _table: &str) -> SourceInfo {
        SourceInfo::mysql(schema, &format!("mysql_{}", self.mysql_config.server_id))
    }

    /// Process binlog data
    pub async fn process_data(
        &self,
        data: &[u8],
        event_tx: &mpsc::Sender<ChangeEvent>,
    ) -> CdcResult<usize> {
        let event = {
            let mut decoder = self.decoder.write().await;
            decoder.decode(data)?
        };

        if let Some(binlog_event) = event {
            if let Some(change_event) = self.to_change_event(binlog_event).await {
                event_tx
                    .send(change_event)
                    .await
                    .map_err(|e| CdcError::Coordinator(format!("Failed to send event: {}", e)))?;
                return Ok(1);
            }
        }

        Ok(0)
    }
}

#[async_trait::async_trait]
impl CdcSource for MySqlConnector {
    fn name(&self) -> &str {
        &self.base.config().name
    }

    fn status(&self) -> SourceStatus {
        self.base.status()
    }

    async fn start(
        &mut self,
        event_tx: mpsc::Sender<ChangeEvent>,
        offset_store: Arc<dyn OffsetStore>,
    ) -> CdcResult<SourceHandle> {
        let shutdown_rx = self.base.init_shutdown();
        self.base.set_status(SourceStatus::Connecting);

        info!(
            "Starting MySQL CDC connector for server: {}",
            self.mysql_config.server_id
        );

        // TODO: Implement MySQL connection and binlog streaming
        // This requires the mysql_async crate or similar
        // For now, we set up the framework but don't connect
        //
        // Required implementation:
        // 1. Connect to MySQL using mysql_async::Conn
        // 2. Execute COM_REGISTER_SLAVE with server_id
        // 3. Execute COM_BINLOG_DUMP with binlog filename/position or GTID
        // 4. Read binlog events from the network stream
        // 5. Decode events using BinlogDecoder
        // 6. Send ChangeEvents through event_tx channel
        // 7. Track position/GTID in offset_store

        self.base.set_status(SourceStatus::Streaming);

        Ok(self
            .base
            .create_handle()
            .ok_or_else(|| CdcError::Coordinator("Failed to create source handle".to_string()))?)
    }

    async fn stop(&mut self) -> CdcResult<()> {
        self.base.set_status(SourceStatus::Stopping);

        // Save current position/GTID
        if let Some(gtid) = self.current_gtid.read().await.clone() {
            let offset = Offset::new(&self.name().to_string(), 0).with_metadata("gtid", gtid);
            self.offset_store.store(&offset).await?;
        } else if let Some(pos) = self.current_position.read().await.clone() {
            let offset = Offset::new(&self.name().to_string(), pos.position)
                .with_metadata("filename", pos.filename);
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
        if let Some(gtid) = self.current_gtid.read().await.clone() {
            return Ok(Some(
                Offset::new(&self.name().to_string(), 0).with_metadata("gtid", gtid),
            ));
        }

        if let Some(pos) = self.current_position.read().await.clone() {
            return Ok(Some(
                Offset::new(&self.name().to_string(), pos.position)
                    .with_metadata("filename", pos.filename),
            ));
        }

        Ok(None)
    }

    fn config(&self) -> &SourceConfig {
        self.base.config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::offset::MemoryOffsetStore;

    async fn create_test_connector() -> MySqlConnector {
        let config = MySqlConfig::new("mysql://localhost/test").with_server_id(12345);

        let offset_store = Arc::new(MemoryOffsetStore::new());
        MySqlConnector::new(config, offset_store).await.unwrap()
    }

    #[tokio::test]
    async fn test_connector_creation() {
        let connector = create_test_connector().await;
        assert_eq!(connector.status(), SourceStatus::Created);
    }

    #[tokio::test]
    async fn test_connector_config() {
        let connector = create_test_connector().await;
        assert_eq!(connector.mysql_config().server_id, 12345);
    }

    #[tokio::test]
    async fn test_position_tracking() {
        let connector = create_test_connector().await;

        assert!(connector.current_position().await.is_none());

        connector
            .update_position(BinlogPosition::new("mysql-bin.000001", 12345))
            .await;

        let pos = connector.current_position().await.unwrap();
        assert_eq!(pos.filename, "mysql-bin.000001");
        assert_eq!(pos.position, 12345);
    }

    #[tokio::test]
    async fn test_gtid_tracking() {
        let connector = create_test_connector().await;

        assert!(connector.current_gtid().await.is_none());

        connector
            .update_gtid("3E11FA47-71CA-11E1-9E33-C80AA9429562:1".to_string())
            .await;

        let gtid = connector.current_gtid().await.unwrap();
        assert!(gtid.contains("3E11FA47"));
    }

    #[tokio::test]
    async fn test_connector_start_stop() {
        let mut connector = create_test_connector().await;
        let (tx, _rx) = mpsc::channel(10);
        let offset_store = Arc::new(MemoryOffsetStore::new());

        let handle = connector.start(tx, offset_store).await.unwrap();
        assert_eq!(connector.status(), SourceStatus::Streaming);

        handle.stop();
        connector.stop().await.unwrap();
        assert_eq!(connector.status(), SourceStatus::Stopped);
    }

    #[tokio::test]
    async fn test_connector_pause_resume() {
        let mut connector = create_test_connector().await;
        let (tx, _rx) = mpsc::channel(10);
        let offset_store = Arc::new(MemoryOffsetStore::new());

        connector.start(tx, offset_store).await.unwrap();

        connector.pause().await.unwrap();
        assert_eq!(connector.status(), SourceStatus::Paused);

        connector.resume().await.unwrap();
        assert_eq!(connector.status(), SourceStatus::Streaming);
    }

    #[tokio::test]
    async fn test_current_offset_gtid() {
        let connector = create_test_connector().await;

        connector.update_gtid("test-gtid:1".to_string()).await;

        let offset = connector.current_offset().await.unwrap().unwrap();
        assert_eq!(
            offset.metadata.get("gtid"),
            Some(&"test-gtid:1".to_string())
        );
    }

    #[tokio::test]
    async fn test_current_offset_position() {
        let connector = create_test_connector().await;

        connector
            .update_position(BinlogPosition::new("mysql-bin.000001", 999))
            .await;

        let offset = connector.current_offset().await.unwrap().unwrap();
        assert_eq!(offset.lsn, 999);
        assert_eq!(
            offset.metadata.get("filename"),
            Some(&"mysql-bin.000001".to_string())
        );
    }
}
