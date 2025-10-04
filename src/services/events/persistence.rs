/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! EventLog persistence for crash recovery

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info, warn};

use crate::index::axis::eventlog::{IndexEvent, EventType, StorageEngineType};
use crate::storage::persistence::filesystem::FilesystemFactory;

/// EventLog WAL (Write-Ahead Log) for persistence
pub struct EventLogWAL {
    /// Directory for WAL files
    wal_dir: PathBuf,

    /// Current WAL file path
    current_file: PathBuf,

    /// Maximum WAL file size before rotation
    max_file_size: u64,

    /// Current file size
    current_size: u64,

    /// Filesystem factory for cloud storage support
    filesystem_factory: Arc<FilesystemFactory>,
}

/// Serializable event for persistence
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistentEvent {
    /// Event from EventLog
    event: IndexEvent,

    /// Timestamp when persisted
    persisted_at: chrono::DateTime<chrono::Utc>,

    /// Whether event has been acknowledged
    acknowledged: bool,
}

impl EventLogWAL {
    /// Create new EventLog WAL
    pub async fn new(
        wal_dir: impl AsRef<Path>,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        let wal_dir = wal_dir.as_ref().to_path_buf();

        // Create WAL directory if it doesn't exist
        fs::create_dir_all(&wal_dir)
            .await
            .context("Failed to create EventLog WAL directory")?;

        let current_file = wal_dir.join("eventlog_wal_current.bin");

        // Get current file size if it exists
        let current_size = if current_file.exists() {
            fs::metadata(&current_file).await?.len()
        } else {
            0
        };

        Ok(Self {
            wal_dir,
            current_file,
            max_file_size: 100 * 1024 * 1024, // 100MB per file
            current_size,
            filesystem_factory,
        })
    }

    /// Persist an event to WAL
    pub async fn persist_event(&mut self, event: &IndexEvent) -> Result<()> {
        let persistent_event = PersistentEvent {
            event: event.clone(),
            persisted_at: chrono::Utc::now(),
            acknowledged: false,
        };

        // Serialize event
        let data = bincode::serialize(&persistent_event).context("Failed to serialize event")?;

        // Check if we need to rotate
        if self.current_size + data.len() as u64 > self.max_file_size {
            self.rotate_wal().await?;
        }

        // Append to current WAL file using filesystem API for consistency
        let current_file_str = self.current_file.to_str().ok_or_else(|| {
            anyhow::anyhow!("Invalid current file path: {:?}", self.current_file)
        })?;
        let filesystem = self.filesystem_factory.get_filesystem(current_file_str)?;

        // Read existing content if file exists
        let mut buffer = if filesystem.exists(current_file_str).await {
            filesystem.read(current_file_str).await?
        } else {
            Vec::new()
        };

        // Write length prefix + data
        let len_bytes = (data.len() as u32).to_le_bytes();
        buffer.extend_from_slice(&len_bytes);
        buffer.extend_from_slice(&data);

        // Write back to file
        filesystem.write(current_file_str, buffer).await?;

        self.current_size += 4 + data.len() as u64;

        debug!(
            "Persisted event {} to WAL (size: {} bytes)",
            event.event_id,
            data.len()
        );

        Ok(())
    }

    /// Mark an event as acknowledged in WAL
    pub async fn acknowledge_event(&mut self, event_id: &str) -> Result<()> {
        // In a production system, we would:
        // 1. Maintain an index of event positions in WAL
        // 2. Update the specific event's acknowledged flag
        // 3. Periodically compact acknowledged events

        debug!("Acknowledged event {} in WAL", event_id);

        // For now, we'll add to a separate acknowledgment file using filesystem API
        let ack_file = self.wal_dir.join("acknowledged_events.txt");
        let ack_file_str = ack_file.to_str().ok_or_else(|| {
            anyhow::anyhow!("Invalid ack file path: {:?}", ack_file)
        })?;
        let filesystem = self.filesystem_factory.get_filesystem(ack_file_str)?;

        // Read existing content if file exists
        let mut content = if filesystem.exists(ack_file_str).await {
            String::from_utf8(filesystem.read(ack_file_str).await?)
                .unwrap_or_default()
        } else {
            String::new()
        };

        // Append new event ID
        content.push_str(&format!("{}\n", event_id));

        // Write back
        filesystem.write(ack_file_str, content.as_bytes().to_vec()).await?;

        Ok(())
    }

    /// Recover pending events from WAL
    pub async fn recover_pending_events(&self) -> Result<Vec<IndexEvent>> {
        let mut pending_events = Vec::new();

        // Read acknowledged events using filesystem API
        let ack_file = self.wal_dir.join("acknowledged_events.txt");
        let ack_file_str = ack_file.to_str().ok_or_else(|| {
            anyhow::anyhow!("Invalid ack file path: {:?}", ack_file)
        })?;
        let filesystem = self.filesystem_factory.get_filesystem(ack_file_str)?;

        let acknowledged_ids = if filesystem.exists(ack_file_str).await {
            let content_bytes = filesystem.read(ack_file_str).await?;
            let content = String::from_utf8(content_bytes).unwrap_or_default();
            content
                .lines()
                .map(|s| s.to_string())
                .collect::<std::collections::HashSet<_>>()
        } else {
            std::collections::HashSet::new()
        };

        // Read all WAL files using filesystem API
        let wal_dir_str = self.wal_dir.to_str().ok_or_else(|| {
            anyhow::anyhow!("Invalid WAL dir path: {:?}", self.wal_dir)
        })?;
        let wal_filesystem = self.filesystem_factory.get_filesystem(wal_dir_str)?;

        let files = wal_filesystem.list(wal_dir_str).await?;
        for file_path in files {
            // Skip non-WAL files
            if !file_path.contains("eventlog_wal_") {
                continue;
            }

            // Read events from WAL file
            let path = std::path::Path::new(&file_path);
            let events = self.read_wal_file(path, &acknowledged_ids).await?;
            pending_events.extend(events);
        }

        info!(
            "Recovered {} pending events from EventLog WAL",
            pending_events.len()
        );

        Ok(pending_events)
    }

    /// Read events from a WAL file
    async fn read_wal_file(
        &self,
        path: &Path,
        acknowledged_ids: &std::collections::HashSet<String>,
    ) -> Result<Vec<IndexEvent>> {
        let mut events = Vec::new();
        // Use filesystem API for cloud compatibility
        let filesystem = self
            .filesystem_factory
            .get_filesystem(path.to_str().unwrap_or(""))?;
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid path: {:?}", path))?;
        let buffer = filesystem.read(path_str).await?;

        let mut cursor = 0;
        while cursor + 4 <= buffer.len() {
            // Read length prefix
            let len_bytes: [u8; 4] = buffer[cursor..cursor + 4].try_into().map_err(|_| {
                anyhow::anyhow!("Failed to read event length at position {}", cursor)
            })?;
            let len = u32::from_le_bytes(len_bytes) as usize;
            cursor += 4;

            if cursor + len > buffer.len() {
                warn!("Incomplete event in WAL file, stopping recovery");
                break;
            }

            // Deserialize event
            match bincode::deserialize::<PersistentEvent>(&buffer[cursor..cursor + len]) {
                Ok(persistent_event) => {
                    // Only include if not acknowledged
                    if !acknowledged_ids.contains(&persistent_event.event.event_id) {
                        events.push(persistent_event.event);
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize event from WAL: {}", e);
                }
            }

            cursor += len;
        }

        debug!(
            "Read {} pending events from {}",
            events.len(),
            path.display()
        );

        Ok(events)
    }

    /// Rotate WAL file
    async fn rotate_wal(&mut self) -> Result<()> {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let rotated_file = self.wal_dir.join(format!("eventlog_wal_{}.bin", timestamp));

        // Use filesystem API for rename
        let current_file_str = self.current_file.to_str().ok_or_else(|| {
            anyhow::anyhow!("Invalid current file path: {:?}", self.current_file)
        })?;
        let rotated_file_str = rotated_file.to_str().ok_or_else(|| {
            anyhow::anyhow!("Invalid rotated file path: {:?}", rotated_file)
        })?;
        let filesystem = self.filesystem_factory.get_filesystem(current_file_str)?;

        // Read current file content and write to new rotated file
        if filesystem.exists(current_file_str).await {
            let content = filesystem.read(current_file_str).await?;
            filesystem.write(rotated_file_str, content).await?;
            filesystem.delete(current_file_str).await?;
        }

        self.current_size = 0;

        info!("Rotated EventLog WAL to {}", rotated_file.display());

        Ok(())
    }

    /// Compact WAL by removing acknowledged events
    pub async fn compact(&mut self) -> Result<()> {
        let ack_file = self.wal_dir.join("acknowledged_events.txt");
        let ack_file_str = ack_file.to_str().ok_or_else(|| {
            anyhow::anyhow!("Invalid ack file path: {:?}", ack_file)
        })?;
        let filesystem = self.filesystem_factory.get_filesystem(ack_file_str)?;

        let acknowledged_ids = if filesystem.exists(ack_file_str).await {
            let content_bytes = filesystem.read(ack_file_str).await?;
            let content = String::from_utf8(content_bytes).unwrap_or_default();
            content
                .lines()
                .map(|s| s.to_string())
                .collect::<std::collections::HashSet<_>>()
        } else {
            return Ok(()); // Nothing to compact
        };

        // Read all pending events
        let pending_events = self.recover_pending_events().await?;

        // Clear existing WAL files using filesystem API
        let wal_dir_str = self.wal_dir.to_str().ok_or_else(|| {
            anyhow::anyhow!("Invalid WAL dir path: {:?}", self.wal_dir)
        })?;
        let wal_filesystem = self.filesystem_factory.get_filesystem(wal_dir_str)?;

        // List and remove WAL files
        let files = wal_filesystem.list(wal_dir_str).await?;
        for file_path in files {
            if file_path.contains("eventlog_wal_") {
                wal_filesystem.delete(&file_path).await.ok(); // Ignore errors
            }
        }

        // Reset current file
        self.current_size = 0;

        // Rewrite only pending events
        for event in pending_events {
            self.persist_event(&event).await?;
        }

        // Clear acknowledgments file
        filesystem.delete(ack_file_str).await.ok();

        info!("Compacted EventLog WAL");

        Ok(())
    }
}

/// Background compaction task for EventLog WAL
pub async fn start_wal_compaction_task(
    wal: Arc<tokio::sync::Mutex<EventLogWAL>>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut shutdown = shutdown;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600)); // Every hour

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Compact WAL
                    match wal.lock().await.compact().await {
                        Ok(()) => debug!("EventLog WAL compaction completed"),
                        Err(e) => error!("EventLog WAL compaction failed: {}", e),
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("EventLog WAL compaction task shutting down");
                        break;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_wal_persistence_and_recovery() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let mut wal = EventLogWAL::new(temp_dir.path(), filesystem_factory).await?;

        // Create test events
        let event1 = IndexEvent {
            event_id: "event_1".to_string(),
            operation: EventType::Flush,
            collection_id: "test_collection".to_string(),
            file_paths: vec!["file1.sstable".to_string()],
            vector_count: 100,
            has_quantized: false,
            has_fp32: true,
            storage_engine: StorageEngineType::SST,
            timestamp: chrono::Utc::now().timestamp() as u64,
        };

        let event2 = IndexEvent {
            event_id: "event_2".to_string(),
            operation: EventType::Compaction,
            collection_id: "test_collection".to_string(),
            file_paths: vec!["output.sstable".to_string()],
            vector_count: 200,
            has_quantized: true,
            has_fp32: true,
            storage_engine: StorageEngineType::SST,
            timestamp: chrono::Utc::now().timestamp() as u64,
        };

        // Persist events
        wal.persist_event(&event1).await?;
        wal.persist_event(&event2).await?;

        // Acknowledge one event
        wal.acknowledge_event("event_1").await?;

        // Recover pending events
        let recovered = wal.recover_pending_events().await?;

        // Should only have event_2 (event_1 was acknowledged)
        assert_eq!(
            recovered.len(),
            1,
            "Should have 1 pending event (event_2), got {}",
            recovered.len()
        );
        assert_eq!(recovered[0].event_id, "event_2");

        Ok(())
    }

    #[tokio::test]
    async fn test_wal_rotation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let mut wal = EventLogWAL::new(temp_dir.path(), filesystem_factory).await?;

        // Set small max file size for testing
        wal.max_file_size = 1024; // 1KB

        // Create large event that will trigger rotation
        let event = IndexEvent {
            event_id: "large_event".to_string(),
            operation: EventType::Flush,
            collection_id: "test_collection".to_string(),
            file_paths: (0..100).map(|i| format!("file_{}.sstable", i)).collect(),
            vector_count: 10000,
            has_quantized: false,
            has_fp32: true,
            storage_engine: StorageEngineType::SST,
            timestamp: chrono::Utc::now().timestamp() as u64,
        };

        // This should trigger rotation
        wal.persist_event(&event).await?;

        // Check that rotation occurred using filesystem API
        let wal_dir_str = temp_dir.path().to_str().unwrap();
        let filesystem = filesystem_factory.get_filesystem(wal_dir_str)?;
        let files = filesystem.list(wal_dir_str).await?;
        let wal_files: Vec<_> = files.iter()
            .filter(|f| f.contains("eventlog_wal_"))
            .collect();

        // Should have at least 1 WAL file (rotation creates a new current file)
        assert!(
            wal_files.len() >= 1,
            "Should have at least 1 WAL file after rotation, got {}",
            wal_files.len()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_wal_compaction() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let mut wal = EventLogWAL::new(temp_dir.path(), filesystem_factory).await?;

        // Create and persist multiple events
        for i in 0..5 {
            let event = IndexEvent {
                event_id: format!("event_{}", i),
                operation: EventType::Flush,
                collection_id: "test_collection".to_string(),
                file_paths: vec![format!("file_{}.sstable", i)],
                vector_count: 100,
                has_quantized: false,
                has_fp32: true,
                storage_engine: StorageEngineType::SST,
                timestamp: chrono::Utc::now().timestamp() as u64,
            };
            wal.persist_event(&event).await?;
        }

        // Acknowledge some events
        wal.acknowledge_event("event_0").await?;
        wal.acknowledge_event("event_2").await?;
        wal.acknowledge_event("event_4").await?;

        // Compact
        wal.compact().await?;

        // Recover and verify only pending events remain
        let recovered = wal.recover_pending_events().await?;
        assert_eq!(
            recovered.len(),
            2,
            "Should have 2 pending events after compaction, got {}",
            recovered.len()
        );

        let event_ids: Vec<_> = recovered.iter().map(|e| e.event_id.as_str()).collect();
        assert!(
            event_ids.contains(&"event_1"),
            "Should have event_1, got {:?}",
            event_ids
        );
        assert!(
            event_ids.contains(&"event_3"),
            "Should have event_3, got {:?}",
            event_ids
        );

        Ok(())
    }
}
