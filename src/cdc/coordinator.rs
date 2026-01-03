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

//! CDC Coordinator
//!
//! The coordinator manages the lifecycle of CDC sources, sinks, and transforms,
//! orchestrating the flow of change events through the pipeline.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, watch};

use super::config::{CdcConfig, SinkConfig, TransformConfig};
use super::error::{CdcError, CdcResult};
use super::event::ChangeEvent;
use super::metrics::CdcMetrics;
use super::offset::{FileOffsetStore, MemoryOffsetStore, OffsetStore};
use super::source::{CdcSource, SourceHandle, SourceStatus};

/// Handle to control the coordinator
#[derive(Clone)]
pub struct CoordinatorHandle {
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
}

impl CoordinatorHandle {
    /// Signal the coordinator to stop
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Coordinator status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatorStatus {
    /// Not started
    Created,
    /// Starting up
    Starting,
    /// Running
    Running,
    /// Stopping
    Stopping,
    /// Stopped
    Stopped,
    /// Error state
    Error,
}

/// CDC Coordinator manages the entire CDC pipeline
pub struct CdcCoordinator {
    /// Configuration
    config: CdcConfig,
    /// Current status
    status: CoordinatorStatus,
    /// Registered sources
    sources: HashMap<String, Arc<RwLock<Box<dyn CdcSource>>>>,
    /// Source handles for control
    source_handles: HashMap<String, SourceHandle>,
    /// Registered sink configurations
    sink_configs: HashMap<String, SinkConfig>,
    /// Registered transform configurations
    transform_configs: HashMap<String, TransformConfig>,
    /// Offset store
    offset_store: Arc<dyn OffsetStore>,
    /// Metrics collector
    metrics: Arc<CdcMetrics>,
    /// Event channel for internal routing
    event_tx: Option<mpsc::Sender<ChangeEvent>>,
    /// Event receiver
    event_rx: Option<mpsc::Receiver<ChangeEvent>>,
    /// Shutdown signal
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Shutdown receiver
    shutdown_rx: Option<watch::Receiver<bool>>,
}

impl CdcCoordinator {
    /// Create a new CDC coordinator with the given configuration
    pub async fn new(config: CdcConfig) -> CdcResult<Self> {
        // Initialize offset store based on config
        let offset_store: Arc<dyn OffsetStore> =
            match config.offset_storage.storage_type {
                super::config::OffsetStorageType::Memory => Arc::new(MemoryOffsetStore::new()),
                super::config::OffsetStorageType::File => {
                    let path =
                        config.offset_storage.path.clone().unwrap_or_else(|| {
                            std::path::PathBuf::from("/tmp/proximadb/cdc/offsets")
                        });
                    Arc::new(FileOffsetStore::new(path).await?)
                }
                super::config::OffsetStorageType::RocksDb => {
                    // Fallback to file for now
                    let path =
                        config.offset_storage.path.clone().unwrap_or_else(|| {
                            std::path::PathBuf::from("/tmp/proximadb/cdc/offsets")
                        });
                    Arc::new(FileOffsetStore::new(path).await?)
                }
            };

        let (event_tx, event_rx) = mpsc::channel(config.settings.queue_size);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let metrics = Arc::new(CdcMetrics::new());

        Ok(Self {
            config,
            status: CoordinatorStatus::Created,
            sources: HashMap::new(),
            source_handles: HashMap::new(),
            sink_configs: HashMap::new(),
            transform_configs: HashMap::new(),
            offset_store,
            metrics,
            event_tx: Some(event_tx),
            event_rx: Some(event_rx),
            shutdown_tx: Some(shutdown_tx),
            shutdown_rx: Some(shutdown_rx),
        })
    }

    /// Get current status
    pub fn status(&self) -> CoordinatorStatus {
        self.status
    }

    /// Get metrics
    pub fn metrics(&self) -> &Arc<CdcMetrics> {
        &self.metrics
    }

    /// Get the offset store
    pub fn offset_store(&self) -> &Arc<dyn OffsetStore> {
        &self.offset_store
    }

    /// Register a source connector
    pub async fn register_source(&mut self, source: Box<dyn CdcSource>) -> CdcResult<()> {
        let name = source.name().to_string();

        if self.sources.contains_key(&name) {
            return Err(CdcError::AlreadyExists(format!(
                "Source '{}' already registered",
                name
            )));
        }

        self.metrics.register_source(&name).await;
        self.sources
            .insert(name.clone(), Arc::new(RwLock::new(source)));

        tracing::info!("Registered CDC source: {}", name);
        Ok(())
    }

    /// Register a sink configuration
    pub async fn register_sink(&mut self, config: SinkConfig) -> CdcResult<()> {
        let name = config.name.clone();

        if self.sink_configs.contains_key(&name) {
            return Err(CdcError::AlreadyExists(format!(
                "Sink '{}' already registered",
                name
            )));
        }

        self.metrics.register_sink(&name).await;
        self.sink_configs.insert(name.clone(), config);

        tracing::info!("Registered CDC sink: {}", name);
        Ok(())
    }

    /// Register a transform configuration
    pub async fn register_transform(&mut self, config: TransformConfig) -> CdcResult<()> {
        let name = config.name.clone();

        if self.transform_configs.contains_key(&name) {
            return Err(CdcError::AlreadyExists(format!(
                "Transform '{}' already registered",
                name
            )));
        }

        self.metrics.register_transform(&name).await;
        self.transform_configs.insert(name.clone(), config);

        tracing::info!("Registered CDC transform: {}", name);
        Ok(())
    }

    /// Start the coordinator and all registered sources
    pub async fn start(&mut self) -> CdcResult<CoordinatorHandle> {
        if self.status != CoordinatorStatus::Created {
            return Err(CdcError::InvalidState(
                "Coordinator already started".to_string(),
            ));
        }

        self.status = CoordinatorStatus::Starting;

        // Get event sender for sources
        let event_tx = self
            .event_tx
            .clone()
            .ok_or_else(|| CdcError::InvalidState("Event channel not initialized".to_string()))?;

        // Start all sources
        for (name, source) in &self.sources {
            let mut source = source.write().await;
            let handle = source
                .start(event_tx.clone(), self.offset_store.clone())
                .await?;
            self.source_handles.insert(name.clone(), handle);
            tracing::info!("Started CDC source: {}", name);
        }

        // Create coordinator handle
        let handle = CoordinatorHandle {
            shutdown_tx: self.shutdown_tx.clone().ok_or_else(|| {
                CdcError::InvalidState("Shutdown channel not initialized".to_string())
            })?,
        };

        self.status = CoordinatorStatus::Running;
        tracing::info!(
            "CDC Coordinator started with {} sources",
            self.sources.len()
        );

        Ok(handle)
    }

    /// Stop the coordinator and all sources
    pub async fn stop(&mut self) -> CdcResult<()> {
        if self.status != CoordinatorStatus::Running {
            return Ok(());
        }

        self.status = CoordinatorStatus::Stopping;

        // Stop all sources
        for (name, handle) in &self.source_handles {
            handle.stop();
            tracing::info!("Stopping CDC source: {}", name);
        }

        // Wait for sources to stop
        for (name, source) in &self.sources {
            let mut source = source.write().await;
            source.stop().await?;
            tracing::info!("Stopped CDC source: {}", name);
        }

        // Flush offset store
        self.offset_store.flush().await?;

        self.status = CoordinatorStatus::Stopped;
        tracing::info!("CDC Coordinator stopped");

        Ok(())
    }

    /// Get source status
    pub async fn source_status(&self, name: &str) -> Option<SourceStatus> {
        self.sources
            .get(name)
            .map(|s| futures::executor::block_on(s.read()).status())
    }

    /// Get all source statuses
    pub async fn all_source_statuses(&self) -> HashMap<String, SourceStatus> {
        let mut statuses = HashMap::new();
        for (name, source) in &self.sources {
            let source = source.read().await;
            statuses.insert(name.clone(), source.status());
        }
        statuses
    }

    /// Process events from the internal queue
    ///
    /// This method should be called in a loop to process incoming events
    pub async fn process_events(&mut self) -> CdcResult<Option<ChangeEvent>> {
        let rx = self
            .event_rx
            .as_mut()
            .ok_or_else(|| CdcError::InvalidState("Event receiver not initialized".to_string()))?;

        match rx.try_recv() {
            Ok(event) => {
                self.metrics.record_event_processed();

                // Record source metrics
                self.metrics
                    .record_source_events(event.source.name(), 1)
                    .await;

                Ok(Some(event))
            }
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Err(CdcError::Coordinator(
                "Event channel disconnected".to_string(),
            )),
        }
    }

    /// Get configuration
    pub fn config(&self) -> &CdcConfig {
        &self.config
    }

    /// Get registered source names
    pub fn source_names(&self) -> Vec<String> {
        self.sources.keys().cloned().collect()
    }

    /// Get registered sink names
    pub fn sink_names(&self) -> Vec<String> {
        self.sink_configs.keys().cloned().collect()
    }

    /// Get registered transform names
    pub fn transform_names(&self) -> Vec<String> {
        self.transform_configs.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::config::OffsetStorageConfig;
    use crate::cdc::event::{ConnectorType, Operation, SourceInfo};
    use crate::cdc::source::MockSource;

    fn test_config() -> CdcConfig {
        CdcConfig {
            offset_storage: OffsetStorageConfig {
                storage_type: super::super::config::OffsetStorageType::Memory,
                path: None,
                flush_interval: std::time::Duration::from_secs(1),
            },
            sources: vec![],
            sinks: vec![],
            transforms: vec![],
            settings: super::super::config::CdcSettings::default(),
        }
    }

    fn create_test_event(id: &str) -> ChangeEvent {
        ChangeEvent::new(
            SourceInfo::new("test_source", ConnectorType::PostgreSQL),
            Operation::Insert,
            "test_collection",
            id,
        )
    }

    #[tokio::test]
    async fn test_coordinator_creation() {
        let coordinator = CdcCoordinator::new(test_config()).await.unwrap();
        assert_eq!(coordinator.status(), CoordinatorStatus::Created);
    }

    #[tokio::test]
    async fn test_register_source() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();

        let source = Box::new(MockSource::new("test_source", vec![]));
        coordinator.register_source(source).await.unwrap();

        assert!(
            coordinator
                .source_names()
                .contains(&"test_source".to_string())
        );
    }

    #[tokio::test]
    async fn test_duplicate_source_registration() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();

        let source1 = Box::new(MockSource::new("test", vec![]));
        let source2 = Box::new(MockSource::new("test", vec![]));

        coordinator.register_source(source1).await.unwrap();
        let result = coordinator.register_source(source2).await;

        assert!(matches!(result, Err(CdcError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_register_sink() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();

        let sink = SinkConfig::kafka("kafka_sink", "localhost:9092");
        coordinator.register_sink(sink).await.unwrap();

        assert!(coordinator.sink_names().contains(&"kafka_sink".to_string()));
    }

    #[tokio::test]
    async fn test_register_transform() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();

        let transform = TransformConfig::vectorization("vectorize");
        coordinator.register_transform(transform).await.unwrap();

        assert!(
            coordinator
                .transform_names()
                .contains(&"vectorize".to_string())
        );
    }

    #[tokio::test]
    async fn test_coordinator_start_stop() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();

        let events = vec![create_test_event("1"), create_test_event("2")];
        let source = Box::new(MockSource::new("mock", events));
        coordinator.register_source(source).await.unwrap();

        let handle = coordinator.start().await.unwrap();
        assert_eq!(coordinator.status(), CoordinatorStatus::Running);

        // Process events
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut processed = 0;
        while let Ok(Some(_event)) = coordinator.process_events().await {
            processed += 1;
        }

        assert_eq!(processed, 2);

        handle.stop();
        coordinator.stop().await.unwrap();
        assert_eq!(coordinator.status(), CoordinatorStatus::Stopped);
    }

    #[tokio::test]
    async fn test_source_status() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();

        let source = Box::new(MockSource::new("mock", vec![]));
        coordinator.register_source(source).await.unwrap();

        let status = coordinator.source_status("mock").await;
        assert_eq!(status, Some(SourceStatus::Created));

        coordinator.start().await.unwrap();

        let status = coordinator.source_status("mock").await;
        assert_eq!(status, Some(SourceStatus::Streaming));
    }

    #[tokio::test]
    async fn test_all_source_statuses() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();

        coordinator
            .register_source(Box::new(MockSource::new("source1", vec![])))
            .await
            .unwrap();
        coordinator
            .register_source(Box::new(MockSource::new("source2", vec![])))
            .await
            .unwrap();

        let statuses = coordinator.all_source_statuses().await;
        assert_eq!(statuses.len(), 2);
    }

    #[tokio::test]
    async fn test_metrics_integration() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();

        let events = vec![create_test_event("1")];
        let source = Box::new(MockSource::new("mock", events));
        coordinator.register_source(source).await.unwrap();

        coordinator.start().await.unwrap();

        // Wait for event processing
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        while let Ok(Some(_)) = coordinator.process_events().await {}

        let metrics = coordinator.metrics().snapshot().await;
        assert!(metrics.total_events >= 1);
    }

    #[tokio::test]
    async fn test_double_start() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();
        coordinator.start().await.unwrap();

        let result = coordinator.start().await;
        assert!(matches!(result, Err(CdcError::InvalidState(_))));
    }

    #[tokio::test]
    async fn test_stop_without_start() {
        let mut coordinator = CdcCoordinator::new(test_config()).await.unwrap();
        // Should not error
        coordinator.stop().await.unwrap();
    }
}
