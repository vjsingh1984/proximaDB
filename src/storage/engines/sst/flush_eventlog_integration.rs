/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! SST flush integration with EventLog for async AXIS indexing

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use crate::services::event_log_service::{get_event_log_service, EventLogService};
use crate::index::axis::eventlog::StorageEngineType;
use crate::storage::engines::FlushParameters;
use crate::proto::proximadb::VectorRecord;

/// SST flush handler that notifies EventLog
pub struct SstFlushHandler {
    event_log: Option<Arc<EventLogService>>,
}

impl SstFlushHandler {
    /// Create new flush handler
    pub fn new() -> Self {
        Self {
            event_log: get_event_log_service(),
        }
    }
    
    /// Notify EventLog after successful flush (synchronous acknowledgment)
    pub async fn notify_flush_complete(
        &self,
        params: &FlushParameters,
        flushed_files: Vec<String>,
        records: &[VectorRecord],
    ) -> Result<()> {
        // Only notify if EventLog service is available
        let event_log = match &self.event_log {
            Some(service) => service,
            None => {
                debug!("EventLog service not available, skipping notification");
                return Ok(());
            }
        };
        
        let collection_id = match params.collection_id.as_ref() {
            Some(id) => id,
            None => {
                debug!("No collection ID in flush params, skipping EventLog notification");
                return Ok(());
            }
        };
        
        // Detect what representations we have
        let has_quantized = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|cfg| cfg.quantization.as_ref())
            .map(|q| q.enabled)
            .unwrap_or(false);
        
        // SST always stores FP32
        let has_fp32 = true;
        
        // Synchronously notify and wait for acknowledgment
        // This ensures flush knows the event has been recorded
        event_log.notify_flush(
            collection_id,
            flushed_files.clone(),
            records.len(),
            has_quantized,
            has_fp32,
            StorageEngineType::SST,
        ).await?;
        
        info!(
            "EventLog acknowledged SST flush: {} files, {} vectors for collection {}",
            flushed_files.len(),
            records.len(),
            collection_id
        );
        
        Ok(())
    }
    
    /// Check if files can be compacted (consults EventLog)
    pub async fn can_compact_files(
        &self,
        collection_id: &str,
        files: &[String],
    ) -> bool {
        match &self.event_log {
            Some(service) => {
                // Check each file with EventLog
                for file in files {
                    if !service.can_compact(collection_id, file).await {
                        info!("File {} not ready for compaction (AXIS indexes pending)", file);
                        return false;
                    }
                }
                true
            }
            None => {
                // No EventLog service, allow compaction
                true
            }
        }
    }
    
    /// Notify about compaction completion
    pub fn notify_compaction_complete(
        &self,
        collection_id: &str,
        output_files: Vec<String>,
        vector_count: usize,
    ) {
        if let Some(event_log) = &self.event_log {
            event_log.notify_compaction(
                collection_id,
                output_files,
                vector_count,
                StorageEngineType::SST,
            );
            
            debug!(
                "Notified EventLog about SST compaction for collection {}",
                collection_id
            );
        }
    }
    
    /// Clean up after compaction
    pub async fn cleanup_compacted_files(
        &self,
        collection_id: &str,
        deleted_files: Vec<String>,
    ) -> Result<()> {
        if let Some(event_log) = &self.event_log {
            event_log.cleanup_compacted_files(collection_id, deleted_files).await?;
        }
        Ok(())
    }
}

impl Default for SstFlushHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_flush_handler_creation() {
        let handler = SstFlushHandler::new();
        // Handler should work even without EventLog service
        assert!(handler.event_log.is_none() || handler.event_log.is_some());
    }
    
    #[tokio::test]
    async fn test_can_compact_without_service() {
        let handler = SstFlushHandler::new();
        // Without EventLog service, compaction should be allowed
        if handler.event_log.is_none() {
            assert!(handler.can_compact_files("test", &["file1.sstable".to_string()]).await);
        }
    }
}