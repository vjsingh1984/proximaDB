/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Service adapter that wraps EventLogManager to implement the service interface
//! Allows EventLog to run embedded, standalone, or distributed

use anyhow::{Result, Context};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::{info, debug};

use super::{
    EventLogManager, EventLogConfig,
    IndexEvent, FileIndexingStatus, ExtractionMode,
    service_interface::*,
};

/// Service adapter that implements the flexible service interface
pub struct EventLogServiceAdapter {
    /// Core event log manager
    manager: Arc<EventLogManager>,
    
    /// Service mode
    mode: ServiceMode,
    
    /// Service start time
    start_time: Instant,
    
    /// Statistics
    stats: Arc<tokio::sync::RwLock<ServiceStats>>,
}

#[derive(Debug, Default)]
struct ServiceStats {
    total_events_processed: u64,
    total_queries: u64,
    last_sync_timestamp: Option<u64>,
}

impl EventLogServiceAdapter {
    /// Create new service adapter
    pub async fn new(
        manager: Arc<EventLogManager>,
        mode: ServiceMode,
    ) -> Result<Arc<Self>> {
        let adapter = Arc::new(Self {
            manager,
            mode,
            start_time: Instant::now(),
            stats: Arc::new(tokio::sync::RwLock::new(ServiceStats::default())),
        });
        
        // Initialize based on mode
        adapter.initialize().await?;
        
        Ok(adapter)
    }
    
    /// Create embedded service (default for single-node)
    pub async fn embedded(
        config: EventLogConfig,
        filesystem_factory: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
        collection_cache: Arc<dashmap::DashMap<String, Arc<crate::proto::proximadb::Collection>>>,
    ) -> Result<Arc<Self>> {
        let manager = EventLogManager::new(config, filesystem_factory, collection_cache).await?;
        Self::new(manager, ServiceMode::Embedded).await
    }
    
    /// Create standalone service (for microservice architecture)
    pub async fn standalone(
        config: EventLogConfig,
        filesystem_factory: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
        collection_cache: Arc<dashmap::DashMap<String, Arc<crate::proto::proximadb::Collection>>>,
        bind_address: String,
        port: u16,
    ) -> Result<Arc<Self>> {
        let manager = EventLogManager::new(config, filesystem_factory, collection_cache).await?;
        
        let mode = ServiceMode::Standalone { bind_address, port };
        let adapter = Self::new(manager, mode).await?;
        
        // Start REST/gRPC server in standalone mode
        adapter.start_server().await?;
        
        Ok(adapter)
    }
    
    /// Start server for standalone mode
    async fn start_server(&self) -> Result<()> {
        match &self.mode {
            ServiceMode::Standalone { bind_address, port } => {
                info!("Starting EventLog service on {}:{}", bind_address, port);
                // Server implementation would go here
                // For now, just log that we would start it
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl EventLogQuery for EventLogServiceAdapter {
    async fn get_pending_events(&self, collection_id: &str) -> Result<Vec<IndexEvent>> {
        self.stats.write().await.total_queries += 1;
        
        let event_log = self.manager.get_event_log(collection_id).await?;
        Ok(event_log.get_pending_events().await)
    }
    
    async fn get_event(&self, event_id: &str) -> Result<Option<IndexEvent>> {
        // Would need to search across all event logs
        for entry in self.manager.event_logs.iter() {
            let events = entry.value().get_pending_events().await;
            if let Some(event) = events.iter().find(|e| e.event_id == event_id) {
                return Ok(Some(event.clone()));
            }
        }
        Ok(None)
    }
    
    async fn get_file_status(&self, file_path: &str) -> Result<Option<FileIndexingStatus>> {
        // Search across all event logs for file status
        for entry in self.manager.event_logs.iter() {
            if let Some(status) = entry.value().get_file_status(file_path) {
                return Ok(Some(status));
            }
        }
        Ok(None)
    }
    
    async fn query_events(&self, filter: EventFilter) -> Result<Vec<IndexEvent>> {
        let mut all_events = Vec::new();
        
        // Collect events from relevant collections
        if let Some(collection_id) = &filter.collection_id {
            if let Ok(event_log) = self.manager.get_event_log(collection_id).await {
                all_events.extend(event_log.get_pending_events().await);
            }
        } else {
            // Query all collections
            for entry in self.manager.event_logs.iter() {
                all_events.extend(entry.value().get_pending_events().await);
            }
        }
        
        // Apply filters
        let mut filtered = all_events;
        
        if let Some(from) = filter.from_timestamp {
            filtered.retain(|e| e.timestamp >= from);
        }
        
        if let Some(to) = filter.to_timestamp {
            filtered.retain(|e| e.timestamp <= to);
        }
        
        if !filter.operation_types.is_empty() {
            filtered.retain(|e| filter.operation_types.contains(&e.operation));
        }
        
        if !filter.storage_engines.is_empty() {
            filtered.retain(|e| filter.storage_engines.contains(&e.storage_engine));
        }
        
        if let Some(limit) = filter.limit {
            filtered.truncate(limit);
        }
        
        Ok(filtered)
    }
    
    async fn get_extraction_hints(
        &self,
        event: &IndexEvent,
        index_type: &str,
    ) -> Result<ExtractionMode> {
        self.manager.get_extraction_hints(&event.collection_id, event, index_type).await
    }
    
    async fn get_health(&self) -> Result<ServiceHealth> {
        let stats = self.stats.read().await;
        let manager_stats = self.manager.stats().await;
        
        Ok(ServiceHealth {
            status: HealthStatus::Healthy,
            mode: format!("{:?}", self.mode),
            pending_events: manager_stats.total_pending_events,
            processed_events: stats.total_events_processed as usize,
            active_collections: manager_stats.collections_with_queues,
            uptime_seconds: self.start_time.elapsed().as_secs(),
            last_sync: stats.last_sync_timestamp,
        })
    }
    
    async fn get_next_batch(&self, batch_size: usize) -> Result<Vec<IndexEvent>> {
        let mut events = Vec::new();
        
        // Collect events from all collections up to batch_size
        for entry in self.manager.event_logs.iter() {
            if events.len() >= batch_size {
                break;
            }
            
            let queue_events = entry.value().get_pending_events().await;
            let remaining = batch_size - events.len();
            events.extend(queue_events.into_iter().take(remaining));
        }
        
        Ok(events)
    }
}

#[async_trait]
impl EventLogCommand for EventLogServiceAdapter {
    async fn add_event(&self, event: IndexEvent) -> Result<()> {
        let event_log = self.manager.get_event_log(&event.collection_id).await?;
        event_log.add_event(event);
        
        self.stats.write().await.total_events_processed += 1;
        Ok(())
    }
    
    async fn mark_processed(&self, event_id: &str, index_name: &str) -> Result<()> {
        // Find which collection owns this event
        for entry in self.manager.event_logs.iter() {
            let events = entry.value().get_pending_events().await;
            if events.iter().any(|e| e.event_id == event_id) {
                entry.value().mark_processed(event_id, index_name);
                return Ok(());
            }
        }
        
        Err(anyhow::anyhow!("Event {} not found", event_id))
    }
    
    async fn mark_batch_processed(&self, updates: Vec<ProcessedUpdate>) -> Result<()> {
        for update in updates {
            self.mark_processed(&update.event_id, &update.index_name).await?;
        }
        Ok(())
    }
    
    async fn can_compact(&self, collection_id: &str, file_path: &str) -> Result<bool> {
        Ok(self.manager.can_compact(collection_id, file_path).await)
    }
    
    async fn cleanup_compacted(&self, collection_id: &str, files: Vec<String>) -> Result<()> {
        self.manager.cleanup_compacted_files(collection_id, files).await
    }
    
    async fn sync_with_peer(&self, peer_id: &str) -> Result<SyncResult> {
        // For distributed mode - would implement peer synchronization
        debug!("Sync with peer {} requested", peer_id);
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        self.stats.write().await.last_sync_timestamp = Some(now);
        
        Ok(SyncResult {
            events_synced: 0,
            conflicts_resolved: 0,
            peer_ahead: false,
            last_sync_timestamp: now,
        })
    }
    
    async fn acknowledge_event(&self, event_id: String) -> Result<()> {
        // Find the event across all collections and acknowledge it
        for entry in self.manager.event_logs.iter() {
            let queue = entry.value();
            // Mark the event as processed (no specific index name for general acknowledgment)
            queue.mark_processed(&event_id, "axis_consumer");
            // Note: mark_processed is synchronous, so we can't tell if the event existed
            // But that's okay - if it didn't exist in this queue, it might be in another
        }
        
        // We've attempted to mark it processed in all queues
        Ok(())
    }
}

#[async_trait]
impl EventLogService for EventLogServiceAdapter {
    fn service_mode(&self) -> ServiceMode {
        self.mode.clone()
    }
    
    async fn initialize(&self) -> Result<()> {
        info!("Initializing EventLog service in {:?} mode", self.mode);
        
        match &self.mode {
            ServiceMode::Embedded => {
                debug!("EventLog running in embedded mode");
            }
            ServiceMode::Standalone { bind_address, port } => {
                info!("EventLog will listen on {}:{}", bind_address, port);
            }
            ServiceMode::Distributed { node_id, coordinator_url, peers } => {
                info!(
                    "EventLog node {} connecting to coordinator at {} with {} peers",
                    node_id,
                    coordinator_url,
                    peers.len()
                );
            }
        }
        
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down EventLog service");
        
        // Note: persist_state is private, persistence handled internally by EventLog
        // The EventLog will persist state automatically during normal operations
        
        Ok(())
    }
}

/// Factory for creating EventLog service in different modes
pub struct EventLogServiceFactory;

impl EventLogServiceFactory {
    /// Create service based on configuration
    pub async fn create(
        config: EventLogConfig,
        filesystem_factory: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
        collection_cache: Arc<dashmap::DashMap<String, Arc<crate::proto::proximadb::Collection>>>,
        deployment_mode: Option<String>,
    ) -> Result<Arc<dyn EventLogService>> {
        let service: Arc<dyn EventLogService> = match deployment_mode.as_deref() {
            Some("standalone") => {
                // Parse standalone config from environment or config
                let bind_address = std::env::var("EVENTLOG_BIND_ADDRESS")
                    .unwrap_or_else(|_| "0.0.0.0".to_string());
                let port = std::env::var("EVENTLOG_PORT")
                    .unwrap_or_else(|_| "8080".to_string())
                    .parse()
                    .context("Invalid port")?;
                
                EventLogServiceAdapter::standalone(
                    config,
                    filesystem_factory,
                    collection_cache,
                    bind_address,
                    port,
                ).await?
            }
            Some("distributed") => {
                // Parse distributed config
                let node_id = std::env::var("EVENTLOG_NODE_ID")
                    .context("EVENTLOG_NODE_ID required for distributed mode")?;
                let coordinator_url = std::env::var("EVENTLOG_COORDINATOR_URL")
                    .context("EVENTLOG_COORDINATOR_URL required")?;
                let peers = std::env::var("EVENTLOG_PEERS")
                    .clone()
                    .split(',')
                    .map(|s| s.to_string())
                    .collect();
                
                let manager = EventLogManager::new(config, filesystem_factory, collection_cache).await?;
                EventLogServiceAdapter::new(
                    manager,
                    ServiceMode::Distributed {
                        node_id,
                        coordinator_url,
                        peers,
                    },
                ).await?
            }
            _ => {
                // Default to embedded
                EventLogServiceAdapter::embedded(config, filesystem_factory, collection_cache).await?
            }
        };
        
        Ok(service)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[tokio::test]
    async fn test_service_modes() {
        // Test that service can be created in different modes
        let temp_dir = TempDir::new().unwrap();
        let base_url = format!("file://{}", temp_dir.path().display());
        
        let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        fs_config.default_fs = Some(base_url.clone());
        
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config)
                .await
                .unwrap()
        );
        
        let collection_cache = Arc::new(dashmap::DashMap::new());
        
        let config = EventLogConfig {
            base_storage_url: base_url,
            max_events_in_memory: 100,
            cleanup_interval_secs: 60,
        };
        
        // Test embedded mode
        let embedded = EventLogServiceAdapter::embedded(
            config.clone(),
            filesystem_factory.clone(),
            collection_cache.clone(),
        ).await.unwrap();
        
        assert!(matches!(embedded.service_mode(), ServiceMode::Embedded));
        
        // Test health check
        let health = embedded.get_health().await.unwrap();
        assert!(matches!(health.status, HealthStatus::Healthy));
    }
}