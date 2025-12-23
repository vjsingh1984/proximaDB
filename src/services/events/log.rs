/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! EventLog service that manages asynchronous indexing coordination
//! Similar to CollectionService, this is a standalone service that recovers on startup

use anyhow::{Context, Result};
use dashmap::DashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::index::axis::eventlog::{
    EventLogConfig, EventLogService as EventLogServiceTrait, EventLogServiceFactory, ServiceMode,
};
use crate::proto::proximadb_v1::Collection;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// EventLog service that coordinates between storage and AXIS indexing
/// This is initialized at server startup like CollectionService and VectorOperationsService
pub struct EventLogService {
    /// The actual event log service implementation
    inner: Arc<dyn EventLogServiceTrait>,

    /// Reference to collection cache (shared with other services)
    collection_cache: Arc<DashMap<String, Arc<Collection>>>,

    /// Filesystem factory for persistence
    filesystem_factory: Arc<FilesystemFactory>,
}

impl EventLogService {
    /// Create and initialize EventLog service
    /// This follows the same pattern as CollectionService::new()
    pub async fn new(
        collection_cache: Arc<DashMap<String, Arc<Collection>>>,
        filesystem_factory: Arc<FilesystemFactory>,
        base_storage_url: Option<String>,
    ) -> Result<Arc<Self>> {
        info!("Initializing EventLog service");

        // Create configuration
        let config = EventLogConfig {
            base_storage_url: base_storage_url
                .unwrap_or_else(|| "file:///data/proximadb".to_string()),
            enable_recovery: true,
            max_events_in_memory: 10000,
            cleanup_interval_secs: 300,
        };

        // Detect deployment mode from environment
        let deployment_mode = std::env::var("EVENTLOG_MODE").ok();

        // Create the event log service
        let inner = EventLogServiceFactory::create(
            config,
            filesystem_factory.clone(),
            collection_cache.clone(),
            deployment_mode.clone(),
        )
        .await
        .context("Failed to create EventLog service")?;

        // Log the mode we're running in
        match inner.service_mode() {
            ServiceMode::Embedded => {
                info!("EventLog service running in embedded mode");
            }
            ServiceMode::Standalone {
                ref bind_address,
                port,
            } => {
                info!(
                    "EventLog service running in standalone mode on {}:{}",
                    bind_address, port
                );
            }
            ServiceMode::Distributed { ref node_id, .. } => {
                info!(
                    "EventLog service running in distributed mode as node {}",
                    node_id
                );
            }
        }

        // Get initial health
        match inner.get_health().await {
            Ok(health) => {
                info!(
                    "EventLog service initialized: {} pending events, {} collections active",
                    health.pending_events, health.active_collections
                );
            }
            Err(e) => {
                warn!("Failed to get EventLog health: {}", e);
            }
        }

        let service = Arc::new(Self {
            inner,
            collection_cache,
            filesystem_factory,
        });

        // The service automatically recovers on creation (like CollectionService)
        // No need for explicit recover() call - it happens in EventLogManager::new()

        Ok(service)
    }

    /// Get the inner service for direct access
    pub fn inner(&self) -> Arc<dyn EventLogServiceTrait> {
        self.inner.clone()
    }

    /// Check if a file can be compacted
    /// This is called by storage engines before compaction
    pub async fn can_compact(&self, collection_id: &str, file_path: &str) -> bool {
        match self.inner.can_compact(collection_id, file_path).await {
            Ok(can_compact) => can_compact,
            Err(e) => {
                // On error, don't block compaction
                debug!(
                    "Error checking compaction status: {}, allowing compaction_info",
                    e
                );
                true
            }
        }
    }

    /// Notify about flush completion (synchronous acknowledgment)
    /// Called by storage engines after flush - waits for EventLog to confirm recording
    pub async fn notify_flush(
        &self,
        collection_id: &str,
        flushed_files: Vec<String>,
        vector_count: usize,
        has_quantized: bool,
        has_fp32: bool,
        storage_engine: crate::index::axis::eventlog::StorageEngineType,
    ) -> Result<()> {
        let event = crate::index::axis::eventlog::IndexEventBuilder::flush_event(
            collection_id.to_string(),
            flushed_files,
            vector_count,
            storage_engine,
            has_quantized,
            has_fp32,
        );

        // Synchronously add event and wait for acknowledgment
        // This ensures flush knows the event has been recorded
        self.inner
            .add_event(event)
            .await
            .context("Failed to record flush event in EventLog")?;

        debug!(
            "Flush event successfully recorded for collection {}",
            collection_id
        );
        Ok(())
    }

    /// Notify about compaction completion (can remain fire-and-forget)
    /// Called by storage engines after compaction
    pub fn notify_compaction(
        &self,
        collection_id: &str,
        output_files: Vec<String>,
        vector_count: usize,
        storage_engine: crate::index::axis::eventlog::StorageEngineType,
    ) {
        let event = crate::index::axis::eventlog::IndexEventBuilder::compaction_event(
            collection_id.to_string(),
            output_files,
            vector_count,
            storage_engine,
        );

        // Compaction notification can remain fire-and-forget
        // since compaction has already completed
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.add_event(event).await {
                debug!("Failed to add compaction event: {}", e);
            }
        });
    }

    /// Cleanup after compaction
    pub async fn cleanup_compacted_files(
        &self,
        collection_id: &str,
        deleted_files: Vec<String>,
    ) -> Result<()> {
        self.inner
            .cleanup_compacted(collection_id, deleted_files)
            .await
    }

    /// Get service statistics
    pub async fn stats(&self) -> EventLogStats {
        match self.inner.get_health().await {
            Ok(health) => EventLogStats {
                pending_events: health.pending_events,
                processed_events: health.processed_events,
                active_collections: health.active_collections,
                uptime_seconds: health.uptime_seconds,
            },
            Err(_) => EventLogStats::default(),
        }
    }

    /// Shutdown the service gracefully
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down EventLog service");
        self.inner.shutdown().await
    }
}

/// EventLog statistics
#[derive(Debug, Default, Clone)]
pub struct EventLogStats {
    pub pending_events: usize,
    pub processed_events: usize,
    pub active_collections: usize,
    pub uptime_seconds: u64,
}

/// Global EventLog service instance (initialized at server startup)
static EVENT_LOG_SERVICE: std::sync::OnceLock<Arc<EventLogService>> = std::sync::OnceLock::new();

/// Global collection cache shared across EventLog service and AXIS consumer
/// This cache is populated when collections are created and used by the consumer
/// to look up collection configs (including index_configs) during flush processing
static GLOBAL_COLLECTION_CACHE: std::sync::OnceLock<Arc<DashMap<String, Arc<Collection>>>> =
    std::sync::OnceLock::new();

/// Get the global collection cache (creates if not exists)
pub fn get_or_create_global_collection_cache() -> Arc<DashMap<String, Arc<Collection>>> {
    GLOBAL_COLLECTION_CACHE
        .get_or_init(|| Arc::new(DashMap::new()))
        .clone()
}

/// Get the global collection cache if initialized
pub fn get_global_collection_cache() -> Option<Arc<DashMap<String, Arc<Collection>>>> {
    GLOBAL_COLLECTION_CACHE.get().cloned()
}

/// Add a collection to the global cache
/// Called when a collection is created to enable EventLog consumer to find it
pub fn register_collection_in_cache(collection: Arc<Collection>) {
    let cache = get_or_create_global_collection_cache();
    let collection_id = collection.id.clone();
    cache.insert(collection_id.clone(), collection);
    tracing::debug!(
        "📦 Registered collection '{}' in global collection cache",
        collection_id
    );
}

/// Remove a collection from the global cache
/// Called when a collection is deleted
pub fn unregister_collection_from_cache(collection_id: &str) {
    if let Some(cache) = get_global_collection_cache() {
        cache.remove(collection_id);
        tracing::debug!(
            "🗑️ Unregistered collection '{}' from global collection cache",
            collection_id
        );
    }
}

/// Initialize the global EventLog service (called once at server startup)
/// This follows the same pattern as collection service initialization
pub async fn initialize_event_log_service(
    collection_cache: Arc<DashMap<String, Arc<Collection>>>,
    filesystem_factory: Arc<FilesystemFactory>,
    base_storage_url: Option<String>,
) -> Result<()> {
    let service =
        EventLogService::new(collection_cache, filesystem_factory, base_storage_url).await?;

    EVENT_LOG_SERVICE
        .set(service)
        .map_err(|_| anyhow::anyhow!("EventLog service already initialized"))?;

    Ok(())
}

/// Get the global EventLog service instance
pub fn event_log_service() -> Option<Arc<EventLogService>> {
    EVENT_LOG_SERVICE.get().cloned()
}

/// Check if EventLog service is initialized
pub fn is_event_log_service_initialized() -> bool {
    EVENT_LOG_SERVICE.get().is_some()
}
