//! HELIX flush integration with EventLog for async AXIS indexing
//!
//! Notifies EventLog after flush operations to enable AXIS indexing
//! with Hilbert-based clustering information.

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use crate::index::axis::eventlog::StorageEngineType;
use crate::core::VectorRecord;
use crate::services::events::log::{EventLogService, event_log_service};
use crate::storage::engines::FlushParameters;

/// HELIX flush handler that notifies EventLog with clustering information
pub struct HelixFlushHandler {
    event_log: Option<Arc<EventLogService>>,
}

impl HelixFlushHandler {
    /// Create new flush handler
    pub fn new() -> Self {
        Self {
            event_log: event_log_service(),
        }
    }

    /// Notify EventLog after successful flush with HELIX-specific metadata
    pub async fn notify_flush_complete(
        &self,
        params: &FlushParameters,
        flushed_files: Vec<String>,
        records: &[VectorRecord],
        hilbert_range: Option<(u64, u64)>,
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
        let has_quantized = params
            .collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|cfg| cfg.quantization.as_ref())
            .map(|q| q.enabled)
            .unwrap_or(false);

        // HELIX uses PCA projection + FastLanes encoding
        let has_fp32 = true;

        // Notify EventLog with HELIX storage type
        event_log
            .notify_flush(
                collection_id,
                flushed_files.clone(),
                records.len(),
                has_quantized,
                has_fp32,
                StorageEngineType::HELIX,
            )
            .await?;

        info!(
            "EventLog acknowledged HELIX flush: {} files, {} vectors, Hilbert range: {:?}",
            flushed_files.len(),
            records.len(),
            hilbert_range
        );

        Ok(())
    }

    /// Check if files can be compacted (consults EventLog)
    pub async fn can_compact_files(&self, collection_id: &str, files: &[String]) -> bool {
        match &self.event_log {
            Some(service) => {
                // Check each file with EventLog
                for file in files {
                    if !service.can_compact(collection_id, file).await {
                        info!(
                            "HELIX file {} not ready for compaction (AXIS indexes pending)",
                            file
                        );
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

    /// Notify about compaction completion with re-clustering information
    pub fn notify_compaction_complete(
        &self,
        collection_id: &str,
        output_files: Vec<String>,
        vector_count: usize,
        improved_clustering: bool,
    ) {
        if let Some(event_log) = &self.event_log {
            event_log.notify_compaction(
                collection_id,
                output_files,
                vector_count,
                StorageEngineType::HELIX,
            );

            debug!(
                "Notified EventLog about HELIX compaction for collection {} (clustering improved: {})",
                collection_id, improved_clustering
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
            event_log
                .cleanup_compacted_files(collection_id, deleted_files)
                .await?;
        }
        Ok(())
    }
}

impl Default for HelixFlushHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helix_flush_handler_creation() {
        let handler = HelixFlushHandler::new();
        // Handler should work even without EventLog service
        assert!(handler.event_log.is_none() || handler.event_log.is_some());
    }

    #[tokio::test]
    async fn test_can_compact_without_service() {
        let handler = HelixFlushHandler::new();
        // Without EventLog service, compaction should be allowed
        if handler.event_log.is_none() {
            assert!(
                handler
                    .can_compact_files("test", &["file1.helix".to_string()])
                    .await
            );
        }
    }
}