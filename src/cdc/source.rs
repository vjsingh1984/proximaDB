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

//! CDC source connectors
//!
//! This module defines the trait and types for CDC source connectors
//! that capture changes from external databases.

use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::watch;

use super::config::SourceConfig;
use super::error::CdcResult;
use super::event::ChangeEvent;
use super::offset::{Offset, OffsetStore};

/// Handle to control a running source
#[derive(Clone)]
pub struct SourceHandle {
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Source name
    name: String,
}

impl SourceHandle {
    /// Create a new source handle
    pub fn new(name: impl Into<String>, shutdown_tx: watch::Sender<bool>) -> Self {
        Self {
            shutdown_tx,
            name: name.into(),
        }
    }

    /// Get the source name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Signal the source to stop
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Source status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStatus {
    /// Not started
    Created,
    /// Connecting to source
    Connecting,
    /// Running snapshot
    Snapshotting,
    /// Streaming changes
    Streaming,
    /// Paused
    Paused,
    /// Stopping
    Stopping,
    /// Stopped
    Stopped,
    /// Error state
    Error,
}

impl std::fmt::Display for SourceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Connecting => write!(f, "connecting"),
            Self::Snapshotting => write!(f, "snapshotting"),
            Self::Streaming => write!(f, "streaming"),
            Self::Paused => write!(f, "paused"),
            Self::Stopping => write!(f, "stopping"),
            Self::Stopped => write!(f, "stopped"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// Trait for CDC source connectors
#[async_trait::async_trait]
pub trait CdcSource: Send + Sync {
    /// Get the source name
    fn name(&self) -> &str;

    /// Get the current status
    fn status(&self) -> SourceStatus;

    /// Start the source and begin streaming events
    ///
    /// # Arguments
    /// * `event_tx` - Channel to send captured events
    /// * `offset_store` - Storage for tracking position
    ///
    /// # Returns
    /// Handle to control the source
    async fn start(
        &mut self,
        event_tx: mpsc::Sender<ChangeEvent>,
        offset_store: Arc<dyn OffsetStore>,
    ) -> CdcResult<SourceHandle>;

    /// Stop the source
    async fn stop(&mut self) -> CdcResult<()>;

    /// Pause the source (stop streaming but maintain connection)
    async fn pause(&mut self) -> CdcResult<()>;

    /// Resume a paused source
    async fn resume(&mut self) -> CdcResult<()>;

    /// Get the current offset
    async fn current_offset(&self) -> CdcResult<Option<Offset>>;

    /// Get source configuration
    fn config(&self) -> &SourceConfig;
}

/// Base implementation for source connectors
pub struct BaseSource {
    /// Source configuration
    config: SourceConfig,
    /// Current status
    status: SourceStatus,
    /// Shutdown receiver
    shutdown_rx: Option<watch::Receiver<bool>>,
    /// Shutdown sender (kept for handle creation)
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Current offset
    current_offset: Option<Offset>,
}

impl BaseSource {
    /// Create a new base source
    pub fn new(config: SourceConfig) -> Self {
        Self {
            config,
            status: SourceStatus::Created,
            shutdown_rx: None,
            shutdown_tx: None,
            current_offset: None,
        }
    }

    /// Initialize shutdown channels
    pub fn init_shutdown(&mut self) -> watch::Receiver<bool> {
        let (tx, rx) = watch::channel(false);
        self.shutdown_tx = Some(tx);
        self.shutdown_rx = Some(rx.clone());
        rx
    }

    /// Create a handle for this source
    pub fn create_handle(&self) -> Option<SourceHandle> {
        self.shutdown_tx
            .clone()
            .map(|tx| SourceHandle::new(self.config.name.clone(), tx))
    }

    /// Get configuration
    pub fn config(&self) -> &SourceConfig {
        &self.config
    }

    /// Get current status
    pub fn status(&self) -> SourceStatus {
        self.status
    }

    /// Set status
    pub fn set_status(&mut self, status: SourceStatus) {
        self.status = status;
    }

    /// Get current offset
    pub fn current_offset(&self) -> Option<&Offset> {
        self.current_offset.as_ref()
    }

    /// Set current offset
    pub fn set_offset(&mut self, offset: Offset) {
        self.current_offset = Some(offset);
    }

    /// Check if shutdown was requested
    pub fn should_shutdown(&self) -> bool {
        self.shutdown_rx.as_ref().is_some_and(|rx| *rx.borrow())
    }

    /// Get shutdown receiver
    pub fn shutdown_rx(&self) -> Option<&watch::Receiver<bool>> {
        self.shutdown_rx.as_ref()
    }
}

/// Mock source for testing
#[cfg(test)]
pub struct MockSource {
    base: BaseSource,
    events_to_emit: Vec<ChangeEvent>,
}

#[cfg(test)]
impl MockSource {
    /// Create a new mock source
    pub fn new(name: &str, events: Vec<ChangeEvent>) -> Self {
        let config = SourceConfig::postgres(name, "mock://localhost");
        Self {
            base: BaseSource::new(config),
            events_to_emit: events,
        }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl CdcSource for MockSource {
    fn name(&self) -> &str {
        &self.base.config.name
    }

    fn status(&self) -> SourceStatus {
        self.base.status()
    }

    async fn start(
        &mut self,
        event_tx: mpsc::Sender<ChangeEvent>,
        _offset_store: Arc<dyn OffsetStore>,
    ) -> CdcResult<SourceHandle> {
        let _shutdown_rx = self.base.init_shutdown();
        self.base.set_status(SourceStatus::Streaming);

        // Emit all events
        for event in &self.events_to_emit {
            let _ = event_tx.send(event.clone()).await;
        }

        Ok(self.base.create_handle().unwrap())
    }

    async fn stop(&mut self) -> CdcResult<()> {
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
        Ok(self.base.current_offset().cloned())
    }

    fn config(&self) -> &SourceConfig {
        self.base.config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::event::{ConnectorType, Operation, SourceInfo};

    fn create_test_event(id: &str) -> ChangeEvent {
        ChangeEvent::new(
            SourceInfo::new("test_source", ConnectorType::PostgreSQL),
            Operation::Insert,
            "test_collection",
            id,
        )
    }

    #[test]
    fn test_source_status_display() {
        assert_eq!(SourceStatus::Created.to_string(), "created");
        assert_eq!(SourceStatus::Streaming.to_string(), "streaming");
        assert_eq!(SourceStatus::Stopped.to_string(), "stopped");
    }

    #[test]
    fn test_base_source_creation() {
        let config = SourceConfig::postgres("test", "postgres://localhost/db");
        let source = BaseSource::new(config);

        assert_eq!(source.status(), SourceStatus::Created);
        assert!(source.current_offset().is_none());
    }

    #[test]
    fn test_base_source_shutdown() {
        let config = SourceConfig::postgres("test", "postgres://localhost/db");
        let mut source = BaseSource::new(config);

        assert!(!source.should_shutdown());

        let _rx = source.init_shutdown();
        assert!(!source.should_shutdown());

        // Signal shutdown
        if let Some(handle) = source.create_handle() {
            handle.stop();
        }

        assert!(source.should_shutdown());
    }

    #[tokio::test]
    async fn test_mock_source() {
        let events = vec![
            create_test_event("1"),
            create_test_event("2"),
            create_test_event("3"),
        ];

        let mut source = MockSource::new("mock", events);
        let (tx, mut rx) = mpsc::channel(10);

        let offset_store = Arc::new(crate::cdc::offset::MemoryOffsetStore::new());
        let handle = source.start(tx, offset_store).await.unwrap();

        assert_eq!(source.status(), SourceStatus::Streaming);

        // Receive events
        let mut received = Vec::new();
        while let Ok(event) = rx.try_recv() {
            received.push(event);
        }

        assert_eq!(received.len(), 3);

        // Stop source
        handle.stop();
        source.stop().await.unwrap();
        assert_eq!(source.status(), SourceStatus::Stopped);
    }

    #[tokio::test]
    async fn test_source_pause_resume() {
        let mut source = MockSource::new("test", vec![]);

        let (tx, _rx) = mpsc::channel(10);
        let offset_store = Arc::new(crate::cdc::offset::MemoryOffsetStore::new());
        let _ = source.start(tx, offset_store).await.unwrap();

        assert_eq!(source.status(), SourceStatus::Streaming);

        source.pause().await.unwrap();
        assert_eq!(source.status(), SourceStatus::Paused);

        source.resume().await.unwrap();
        assert_eq!(source.status(), SourceStatus::Streaming);
    }

    #[test]
    fn test_source_handle() {
        let (tx, _rx) = watch::channel(false);
        let handle = SourceHandle::new("test_source", tx);

        assert_eq!(handle.name(), "test_source");
    }
}
