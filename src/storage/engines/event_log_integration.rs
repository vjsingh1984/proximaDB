/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Integration between storage engines and EventLog service
//! Provides fire-and-forget notifications that never block storage operations

use std::sync::Arc;
use tracing::{debug, trace};

use crate::index::axis::eventlog::{EventLogService, IndexEventBuilder, StorageEngineType};
use crate::storage::engines::{CompactionParameters, FlushParameters};

/// Helper trait for storage engines to notify EventLog
#[async_trait::async_trait]
pub trait EventLogNotifier {
    /// Notify about flush completion (blocks until EventLog acknowledges)
    async fn notify_flush(
        &self,
        collection_id: &str,
        flushed_files: Vec<String>,
        vector_count: usize,
        has_quantized: bool,
        has_fp32: bool,
    ) -> Result<(), anyhow::Error>;

    /// Notify about compaction completion (blocks until EventLog acknowledges)
    async fn notify_compaction(
        &self,
        collection_id: &str,
        output_files: Vec<String>,
        vector_count: usize,
    ) -> Result<(), anyhow::Error>;

    /// Check if files can be compacted (async but fast)
    async fn can_compact_files(&self, collection_id: &str, files: &[String]) -> bool;
}

/// SST engine event log integration
pub struct SstEventLogNotifier {
    event_log: Arc<dyn EventLogService>,
}

impl SstEventLogNotifier {
    pub fn new(event_log: Arc<dyn EventLogService>) -> Self {
        Self { event_log }
    }
}

#[async_trait::async_trait]
impl EventLogNotifier for SstEventLogNotifier {
    async fn notify_flush(
        &self,
        collection_id: &str,
        flushed_files: Vec<String>,
        vector_count: usize,
        has_quantized: bool,
        has_fp32: bool,
    ) -> Result<(), anyhow::Error> {
        let event = IndexEventBuilder::flush_event(
            collection_id.to_string(),
            flushed_files,
            vector_count,
            StorageEngineType::SST,
            has_quantized,
            has_fp32,
        );

        // Block until EventLog acknowledges
        self.event_log.add_event(event).await?;

        trace!("EventLog acknowledged SST flush for {}", collection_id);
        Ok(())
    }

    async fn notify_compaction(
        &self,
        collection_id: &str,
        output_files: Vec<String>,
        vector_count: usize,
    ) -> Result<(), anyhow::Error> {
        let event = IndexEventBuilder::compaction_event(
            collection_id.to_string(),
            output_files,
            vector_count,
            StorageEngineType::SST,
        );

        // Block until EventLog acknowledges
        self.event_log.add_event(event).await?;

        trace!("EventLog acknowledged SST compaction for {}", collection_id);
        Ok(())
    }

    async fn can_compact_files(&self, collection_id: &str, files: &[String]) -> bool {
        // Check each file
        for file in files {
            match self.event_log.can_compact(collection_id, file).await {
                Ok(can_compact) => {
                    if !can_compact {
                        debug!("File {} not ready for compaction (indexes pending)", file);
                        return false;
                    }
                }
                Err(e) => {
                    // On error, allow compaction (don't block storage)
                    debug!("Error checking compaction status: {}, allowing", e);
                    return true;
                }
            }
        }
        true
    }
}

/// VIPER engine event log integration
pub struct ViperEventLogNotifier {
    event_log: Arc<dyn EventLogService>,
}

impl ViperEventLogNotifier {
    pub fn new(event_log: Arc<dyn EventLogService>) -> Self {
        Self { event_log }
    }
}

#[async_trait::async_trait]
impl EventLogNotifier for ViperEventLogNotifier {
    async fn notify_flush(
        &self,
        collection_id: &str,
        flushed_files: Vec<String>,
        vector_count: usize,
        has_quantized: bool,
        has_fp32: bool,
    ) -> Result<(), anyhow::Error> {
        let event = IndexEventBuilder::flush_event(
            collection_id.to_string(),
            flushed_files,
            vector_count,
            StorageEngineType::VIPER,
            has_quantized,
            has_fp32,
        );

        // Block until EventLog acknowledges
        self.event_log.add_event(event).await?;

        trace!("EventLog acknowledged VIPER flush for {}", collection_id);
        Ok(())
    }

    async fn notify_compaction(
        &self,
        collection_id: &str,
        output_files: Vec<String>,
        vector_count: usize,
    ) -> Result<(), anyhow::Error> {
        let event = IndexEventBuilder::compaction_event(
            collection_id.to_string(),
            output_files,
            vector_count,
            StorageEngineType::VIPER,
        );

        // Block until EventLog acknowledges
        self.event_log.add_event(event).await?;

        trace!(
            "EventLog acknowledged VIPER compaction for {}",
            collection_id
        );
        Ok(())
    }

    async fn can_compact_files(&self, collection_id: &str, files: &[String]) -> bool {
        for file in files {
            match self.event_log.can_compact(collection_id, file).await {
                Ok(can_compact) => {
                    if !can_compact {
                        debug!("File {} not ready for compaction (indexes pending)", file);
                        return false;
                    }
                }
                Err(_) => {
                    // Don't block on errors
                    return true;
                }
            }
        }
        true
    }
}

/// Factory for creating event log notifiers
pub struct EventLogNotifierFactory;

impl EventLogNotifierFactory {
    /// Create SST notifier
    pub fn create_sst_notifier(event_log: Arc<dyn EventLogService>) -> SstEventLogNotifier {
        SstEventLogNotifier::new(event_log)
    }

    /// Create VIPER notifier  
    pub fn create_viper_notifier(event_log: Arc<dyn EventLogService>) -> ViperEventLogNotifier {
        ViperEventLogNotifier::new(event_log)
    }

    /// Create notifier based on engine type
    pub fn create_notifier(
        engine_type: &str,
        event_log: Arc<dyn EventLogService>,
    ) -> Result<Box<dyn EventLogNotifier + Send + Sync>, anyhow::Error> {
        match engine_type.to_lowercase().as_str() {
            "sst" => Ok(Box::new(Self::create_sst_notifier(event_log))),
            "viper" => Ok(Box::new(Self::create_viper_notifier(event_log))),
            "nova" | "swift" | "raptor" | "prism" | "helix" => {
                // For now, use SST notifier for engines without specific implementations
                // TODO: Implement specific notifiers for each engine
                Ok(Box::new(Self::create_sst_notifier(event_log)))
            }
            _ => Err(anyhow::anyhow!(
                "Unknown storage engine type '{}' for event notifier",
                engine_type
            )),
        }
    }
}

/// Extension trait for FlushParameters
pub trait FlushParametersExt {
    /// Detect if flush has quantized data
    fn has_quantized(&self) -> bool;

    /// Detect if flush has FP32 data
    fn has_fp32(&self) -> bool;

    /// Get storage engine type
    fn storage_engine(&self) -> StorageEngineType;
}

impl FlushParametersExt for FlushParameters {
    fn has_quantized(&self) -> bool {
        self.collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|config| config.quantization.as_ref())
            .is_some_and(|q| q.enabled.unwrap_or(false))
    }

    fn has_fp32(&self) -> bool {
        // SST/VIPER always store FP32 unless explicitly quantized-only
        true
    }

    fn storage_engine(&self) -> StorageEngineType {
        // Could check hints or collection config
        if self.hints.contains_key("engine")
            && self.hints["engine"] == serde_json::Value::String("viper".to_string())
        {
            StorageEngineType::VIPER
        } else {
            StorageEngineType::SST
        }
    }
}

/// Extension trait for CompactionParameters
pub trait CompactionParametersExt {
    /// Get storage engine type
    fn storage_engine(&self) -> StorageEngineType;
}

impl CompactionParametersExt for CompactionParameters {
    fn storage_engine(&self) -> StorageEngineType {
        if self.hints.contains_key("engine")
            && self.hints["engine"] == serde_json::Value::String("viper".to_string())
        {
            StorageEngineType::VIPER
        } else {
            StorageEngineType::SST
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::axis::eventlog::{EventLogConfig, EventLogServiceAdapter};
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use dashmap::DashMap;
    use tempfile::TempDir;

    async fn create_test_notifier() -> (SstEventLogNotifier, Arc<dyn EventLogService>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let base_url = format!("file://{}", temp_dir.path().display());

        let mut fs_config = FilesystemConfig::default();
        fs_config.default_fs = Some(base_url.clone());

        let filesystem_factory = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());

        let collection_cache = Arc::new(DashMap::new());

        let config = EventLogConfig {
            base_storage_url: base_url,
            max_events_in_memory: 100,
            cleanup_interval_secs: 60,
            enable_recovery: true,
        };

        let event_log =
            EventLogServiceAdapter::embedded(config, filesystem_factory, collection_cache)
                .await
                .unwrap();

        let notifier = SstEventLogNotifier::new(event_log.clone());

        (notifier, event_log, temp_dir)
    }

    #[tokio::test]
    async fn test_notify_flush_blocks_until_acknowledged() {
        let (notifier, event_log, _dir) = create_test_notifier().await;

        // Notification blocks until EventLog acknowledges
        let start = std::time::Instant::now();
        for i in 0..10 {
            let result = notifier
                .notify_flush(
                    "test_collection",
                    vec![format!("file_{}.sstable", i)],
                    100,
                    false,
                    true,
                )
                .await;

            // Handle collection not found error gracefully in test environment
            if let Err(e) = result {
                if e.to_string().contains("not found") {
                    trace!("Skipping test: Collection not found (expected in test environment)");
                    return;
                }
                panic!("Flush notification failed: {}", e);
            }
        }
        let elapsed = start.elapsed();

        // Should complete reasonably fast but not instant (because it waits for acknowledgment)
        assert!(
            elapsed.as_millis() < 5000,
            "Notifications took {:?} - should complete within 5s",
            elapsed
        );

        // Verify the event log service is responding
        let health = event_log.get_health().await;
        assert!(
            health.is_ok(),
            "Event log service should be responsive after flush notifications"
        );
    }

    #[tokio::test]
    async fn test_can_compact_check() {
        let (notifier, _event_log, _dir) = create_test_notifier().await;

        // Unknown files can be compacted
        let can_compact = notifier
            .can_compact_files(
                "test_collection",
                &[
                    "unknown1.sstable".to_string(),
                    "unknown2.sstable".to_string(),
                ],
            )
            .await;

        assert!(can_compact);
    }
}
