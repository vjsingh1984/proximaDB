/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! VIPER flush integration with EventLog for async AXIS indexing

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use crate::services::events::log::{event_log_service, EventLogService};
use crate::index::axis::eventlog::StorageEngineType;
use crate::storage::engines::FlushParameters;
use crate::proto::proximadb::VectorRecord;

/// VIPER flush event notifier for EventLog integration
pub struct ViperFlushNotifier {
    event_log: Option<Arc<EventLogService>>,
}

impl ViperFlushNotifier {
    /// Create new flush handler
    pub fn new() -> Self {
        Self {
            event_log: event_log_service(),
        }
    }
    
    /// Notify EventLog after successful flush (synchronous acknowledgment)
    pub async fn notify_flush_complete(
        &self,
        params: &FlushParameters,
        flushed_files: Vec<String>,
        records: &[VectorRecord],
    ) -> Result<()> {
        let event_log = match &self.event_log {
            Some(service) => service,
            None => {
                debug!("EventLog service not available, skipping notification");
                return Ok(());
            }
        };
        
        let collection_id = match params.collection_id.as_ref() {
            Some(id) => id,
            None => return Ok(()),
        };
        
        // VIPER stores both FP32 (in one column) and quantized (in another)
        let has_quantized = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|cfg| cfg.quantization.as_ref())
            .map(|q| q.enabled)
            ;
        
        let has_fp32 = true; // VIPER always has FP32 column
        
        // Synchronously notify and wait for acknowledgment
        // This ensures flush knows the event has been recorded
        event_log.notify_flush(
            collection_id,
            flushed_files.clone(),
            records.len(),
            has_quantized.unwrap_or(false),
            has_fp32,
            StorageEngineType::VIPER,
        ).await?;
        
        info!(
            "EventLog acknowledged VIPER flush: {} files, {} vectors for collection {}",
            flushed_files.len(),
            records.len(),
            collection_id
        );
        
        Ok(())
    }
    
    /// Check if files can be compacted
    pub async fn can_compact_files(
        &self,
        collection_id: &str,
        files: &[String],
    ) -> bool {
        match &self.event_log {
            Some(service) => {
                for file in files {
                    if !service.can_compact(collection_id, file).await {
                        info!("Parquet file {} not ready for compaction (AXIS indexes pending)", file);
                        return false;
                    }
                }
                true
            }
            None => true,
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
                StorageEngineType::VIPER,
            );
            
            debug!(
                "Notified EventLog about VIPER compaction for collection {}",
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

impl Default for ViperFlushNotifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Alias for consistency with other engines
pub type ViperFlushHandler = ViperFlushNotifier;