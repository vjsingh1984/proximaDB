/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Shared service for metadata queue management and recovery

use anyhow::{Context, Result};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::event_log::{
    EventLogQueue, ExtractionMode, IndexEvent, IndexEventBuilder, StorageEngineType,
};
use crate::proto::proximadb_v1::Collection;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Configuration for metadata queue service
#[derive(Debug, Clone)]
pub struct EventLogConfig {
    /// Base URL for queue storage (e.g., s3://bucket/proximadb/)
    pub base_storage_url: String,

    /// Enable automatic recovery on startup
    pub enable_recovery: bool,

    /// Maximum events to keep in memory per collection
    pub max_events_in_memory: usize,

    /// Cleanup interval for processed events
    pub cleanup_interval_secs: u64,
}

impl Default for EventLogConfig {
    fn default() -> Self {
        Self {
            base_storage_url: "file:///data/proximadb".to_string(),
            enable_recovery: true,
            max_events_in_memory: 10000,
            cleanup_interval_secs: 300, // 5 minutes
        }
    }
}

/// Event log manager that recovers independently at startup
/// Similar to CollectionService, maintains its own recovery lifecycle
pub struct EventLogManager {
    /// Configuration
    config: EventLogConfig,

    /// Filesystem factory for cloud storage
    filesystem_factory: Arc<FilesystemFactory>,

    /// Per-collection queues
    pub event_logs: Arc<DashMap<String, Arc<EventLogQueue>>>,

    /// Shared collection cache (from VectorOperationsService)
    shared_collection_cache: Arc<DashMap<String, Arc<Collection>>>,

    /// Service state
    initialized: Arc<RwLock<bool>>,
}

impl EventLogManager {
    /// Create and recover event log manager
    /// This follows the same pattern as CollectionService::new()
    pub async fn new(
        config: EventLogConfig,
        filesystem_factory: Arc<FilesystemFactory>,
        shared_collection_cache: Arc<DashMap<String, Arc<Collection>>>,
    ) -> Result<Arc<Self>> {
        let service = Arc::new(Self {
            config,
            filesystem_factory,
            event_logs: Arc::new(DashMap::new()),
            shared_collection_cache,
            initialized: Arc::new(RwLock::new(false)),
        });

        // Always recover on startup (like CollectionService)
        service.recover_all_event_logs().await?;

        *service.initialized.write().await = true;

        // Start cleanup task
        service.start_cleanup_task();

        Ok(service)
    }

    /// Get or create queue for a collection
    pub async fn get_event_log(&self, collection_id: &str) -> Result<Arc<EventLogQueue>> {
        // Check if event log already exists
        if let Some(log) = self.event_logs.get(collection_id) {
            return Ok(log.clone());
        }

        // Get collection from cache to determine storage location
        let collection = self
            .shared_collection_cache
            .get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("Collection {} not found", collection_id))?;

        // Determine queue storage URL
        let queue_url = self.get_queue_url(&collection);

        // Create new event log
        let event_log = Arc::new(
            EventLogQueue::new(
                queue_url,
                collection_id.to_string(),
                self.filesystem_factory.clone(),
            )
            .await
            .context("Failed to create event log")?,
        );

        self.event_logs
            .insert(collection_id.to_string(), event_log.clone());

        info!("Created event log for collection {}", collection_id);
        Ok(event_log)
    }

    /// Add flush event (called by storage engines)
    pub async fn add_flush_event(
        &self,
        collection_id: &str,
        file_paths: Vec<String>,
        vector_count: usize,
        storage_engine: StorageEngineType,
        has_quantized: bool,
        has_fp32: bool,
    ) -> Result<()> {
        let event_log = self.get_event_log(collection_id).await?;

        let event = IndexEventBuilder::flush_event(
            collection_id.to_string(),
            file_paths,
            vector_count,
            storage_engine,
            has_quantized,
            has_fp32,
        );

        event_log.add_event(event);
        debug!("Added flush event for collection {}", collection_id);
        Ok(())
    }

    /// Add compaction event
    pub async fn add_compaction_event(
        &self,
        collection_id: &str,
        output_files: Vec<String>,
        vector_count: usize,
        storage_engine: StorageEngineType,
    ) -> Result<()> {
        let event_log = self.get_event_log(collection_id).await?;

        let event = IndexEventBuilder::compaction_event(
            collection_id.to_string(),
            output_files,
            vector_count,
            storage_engine,
        );

        event_log.add_event(event);
        debug!("Added compaction event for collection {}", collection_id);
        Ok(())
    }

    /// Check if file can be compacted
    pub async fn can_compact(&self, collection_id: &str, file_path: &str) -> bool {
        match self.get_event_log(collection_id).await {
            Ok(log) => log.can_compact(file_path),
            Err(_) => true, // If no event log, allow compaction
        }
    }

    /// Clean up after compaction
    pub async fn cleanup_compacted_files(
        &self,
        collection_id: &str,
        deleted_files: Vec<String>,
    ) -> Result<()> {
        let event_log = self.get_event_log(collection_id).await?;
        event_log.cleanup_compacted_files(deleted_files);
        Ok(())
    }

    /// Get extraction hints for AXIS
    pub async fn get_extraction_hints(
        &self,
        collection_id: &str,
        event: &IndexEvent,
        index_type: &str,
    ) -> Result<ExtractionMode> {
        let event_log = self.get_event_log(collection_id).await?;
        Ok(event_log.get_extraction_hints(event, index_type))
    }

    /// Recover all event logs on startup (called automatically in new())
    async fn recover_all_event_logs(&self) -> Result<()> {
        info!("Starting event log recovery");

        let mut recovered_count = 0;
        let mut failed_count = 0;

        // Iterate through all collections in cache
        for entry in self.shared_collection_cache.iter() {
            let collection_id = entry.key();
            let collection = entry.value();

            match self
                .recover_queue_for_collection(collection_id, collection)
                .await
            {
                Ok(_) => {
                    recovered_count += 1;
                    debug!("Recovered queue for collection {}", collection_id);
                }
                Err(e) => {
                    failed_count += 1;
                    warn!(
                        "Failed to recover queue for collection {}: {}",
                        collection_id, e
                    );
                }
            }
        }

        info!(
            "Queue recovery complete: {} recovered, {} failed",
            recovered_count, failed_count
        );

        Ok(())
    }

    /// Recover queue for a specific collection
    async fn recover_queue_for_collection(
        &self,
        collection_id: &str,
        collection: &Arc<Collection>,
    ) -> Result<()> {
        let queue_url = self.get_queue_url(collection);

        let queue = Arc::new(
            EventLogQueue::new(
                queue_url,
                collection_id.to_string(),
                self.filesystem_factory.clone(),
            )
            .await?,
        );

        self.event_logs.insert(collection_id.to_string(), queue);
        Ok(())
    }

    /// Get queue URL for a collection
    fn get_queue_url(&self, collection: &Arc<Collection>) -> String {
        // Use collection's storage assignment if available
        if let Some(storage_assignment) = &collection.storage_assignment {
            format!("{}/queue", storage_assignment.base_location)
        } else {
            // Fall back to default
            format!("{}/queue", self.config.base_storage_url)
        }
    }

    /// Start background cleanup task
    fn start_cleanup_task(&self) {
        let service = Arc::new(self.clone_refs());
        let cleanup_interval = self.config.cleanup_interval_secs;

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(cleanup_interval));

            loop {
                interval.tick().await;

                // Clean up old events from all queues
                for entry in service.event_logs.iter() {
                    let collection_id = entry.key();
                    let queue = entry.value();

                    // Get pending events count
                    let pending_count = queue.get_pending_events().await.len();

                    if pending_count > service.config.max_events_in_memory {
                        debug!(
                            "Queue for {} has {} events, considering cleanup",
                            collection_id, pending_count
                        );
                        // Queue handles its own cleanup internally
                    }
                }
            }
        });
    }

    /// Clone service references for async tasks
    fn clone_refs(&self) -> EventLogManager {
        EventLogManager {
            config: self.config.clone(),
            filesystem_factory: self.filesystem_factory.clone(),
            event_logs: self.event_logs.clone(),
            shared_collection_cache: self.shared_collection_cache.clone(),
            initialized: self.initialized.clone(),
        }
    }

    /// Check if service is initialized
    pub async fn is_initialized(&self) -> bool {
        *self.initialized.read().await
    }

    /// Get statistics for monitoring
    pub async fn get_stats(&self) -> QueueServiceStats {
        let mut total_events = 0;
        let mut total_files_tracked = 0;

        for entry in self.event_logs.iter() {
            let queue = entry.value();
            let events = queue.get_pending_events().await;
            total_events += events.len();

            // Count unique files
            let mut files = std::collections::HashSet::new();
            for event in events {
                for file in event.file_paths {
                    files.insert(file);
                }
            }
            total_files_tracked += files.len();
        }

        QueueServiceStats {
            collections_with_queues: self.event_logs.len(),
            total_pending_events: total_events,
            total_files_tracked: total_files_tracked,
        }
    }
}

/// Service statistics
#[derive(Debug, Clone)]
pub struct QueueServiceStats {
    pub collections_with_queues: usize,
    pub total_pending_events: usize,
    pub total_files_tracked: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_service() -> (Arc<EventLogManager>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let base_url = format!("file://{}", temp_dir.path().display());

        let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        fs_config.default_fs = Some(base_url.clone());

        let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());

        let collection_cache = Arc::new(DashMap::new());

        // Add test collection
        let mut collection = Collection::default();
        collection.id = "test_collection".to_string();
        collection_cache.insert("test_collection".to_string(), Arc::new(collection));

        let config = EventLogConfig {
            base_storage_url: base_url,
            enable_recovery: true,
            max_events_in_memory: 100,
            cleanup_interval_secs: 60,
        };

        let service = EventLogManager::new(config, filesystem_factory, collection_cache)
            .await
            .unwrap();

        (service, temp_dir)
    }

    #[tokio::test]
    async fn test_service_initialization() {
        let (service, _dir) = create_test_service().await;
        assert!(service.is_initialized().await);
    }

    #[tokio::test]
    async fn test_add_flush_event() {
        let (service, _dir) = create_test_service().await;

        service
            .add_flush_event(
                "test_collection",
                vec!["file1.sstable".to_string()],
                1000,
                StorageEngineType::SST,
                false,
                true,
            )
            .await
            .unwrap();

        let stats = service.stats().await;
        assert_eq!(stats.collections_with_queues, 1);
        assert_eq!(stats.total_pending_events, 1);
    }

    #[tokio::test]
    async fn test_compaction_check() {
        let (service, _dir) = create_test_service().await;

        // Add event
        service
            .add_flush_event(
                "test_collection",
                vec!["file1.sstable".to_string()],
                500,
                StorageEngineType::SST,
                true,
                true,
            )
            .await
            .unwrap();

        // Initially not ready for compaction
        assert!(
            !service
                .can_compact("test_collection", "file1.sstable")
                .await
        );
    }
}
