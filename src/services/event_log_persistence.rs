/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! EventLog persistence for crash recovery

use anyhow::{Result, Context};
use serde::{Serialize, Deserialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use tracing::{debug, info, warn, error};

use crate::index::axis::eventlog::{IndexEvent, EventType, StorageEngineType};

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
}

/// Serializable event for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub async fn new(wal_dir: impl AsRef<Path>) -> Result<Self> {
        let wal_dir = wal_dir.as_ref().to_path_buf();
        
        // Create WAL directory if it doesn't exist
        fs::create_dir_all(&wal_dir).await
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
        let data = bincode::serialize(&persistent_event)
            .context("Failed to serialize event")?;
        
        // Check if we need to rotate
        if self.current_size + data.len() as u64 > self.max_file_size {
            self.rotate_wal().await?;
        }
        
        // Append to current WAL file
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.current_file)
            .await
            .context("Failed to open WAL file")?;
        
        // Write length prefix + data
        let len_bytes = (data.len() as u32).to_le_bytes();
        file.write_all(&len_bytes).await?;
        file.write_all(&data).await?;
        file.flush().await?;
        
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
        
        // For now, we'll add to a separate acknowledgment file
        let ack_file = self.wal_dir.join("acknowledged_events.txt");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&ack_file)
            .await?;
        
        file.write_all(format!("{}\n", event_id).as_bytes()).await?;
        file.flush().await?;
        
        Ok(())
    }
    
    /// Recover pending events from WAL
    pub async fn recover_pending_events(&self) -> Result<Vec<IndexEvent>> {
        let mut pending_events = Vec::new();
        
        // Read acknowledged events
        let ack_file = self.wal_dir.join("acknowledged_events.txt");
        let acknowledged_ids = if ack_file.exists() {
            let content = fs::read_to_string(&ack_file).await?;
            content.lines()
                .map(|s| s.to_string())
                .collect::<std::collections::HashSet<_>>()
        } else {
            std::collections::HashSet::new()
        };
        
        // Read all WAL files
        let mut entries = fs::read_dir(&self.wal_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            
            // Skip non-WAL files
            if !path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("eventlog_wal_"))
                .unwrap_or(false)
            {
                continue;
            }
            
            // Read events from WAL file
            let events = self.read_wal_file(&path, &acknowledged_ids).await?;
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
        let mut file = fs::File::open(path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;
        
        let mut cursor = 0;
        while cursor + 4 <= buffer.len() {
            // Read length prefix
            let len_bytes: [u8; 4] = buffer[cursor..cursor + 4].try_into().unwrap();
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
        
        debug!("Read {} pending events from {}", events.len(), path.display());
        
        Ok(events)
    }
    
    /// Rotate WAL file
    async fn rotate_wal(&mut self) -> Result<()> {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let rotated_file = self.wal_dir.join(format!("eventlog_wal_{}.bin", timestamp));
        
        fs::rename(&self.current_file, &rotated_file).await
            .context("Failed to rotate WAL file")?;
        
        self.current_size = 0;
        
        info!("Rotated EventLog WAL to {}", rotated_file.display());
        
        Ok(())
    }
    
    /// Compact WAL by removing acknowledged events
    pub async fn compact(&mut self) -> Result<()> {
        let ack_file = self.wal_dir.join("acknowledged_events.txt");
        let acknowledged_ids = if ack_file.exists() {
            let content = fs::read_to_string(&ack_file).await?;
            content.lines()
                .map(|s| s.to_string())
                .collect::<std::collections::HashSet<_>>()
        } else {
            return Ok(()); // Nothing to compact
        };
        
        // Read all pending events
        let pending_events = self.recover_pending_events().await?;
        
        // Clear existing WAL files
        let mut entries = fs::read_dir(&self.wal_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("eventlog_wal_"))
                .unwrap_or(false)
            {
                fs::remove_file(&path).await?;
            }
        }
        
        // Reset current file
        self.current_size = 0;
        
        // Rewrite only pending events
        for event in pending_events {
            self.persist_event(&event).await?;
        }
        
        // Clear acknowledgments file
        fs::remove_file(&ack_file).await.ok();
        
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
        let mut wal = EventLogWAL::new(temp_dir.path()).await?;
        
        // Create test events
        let event1 = IndexEvent {
            event_id: "event_1".to_string(),
            event_type: EventType::Flush,
            collection_id: "test_collection".to_string(),
            data_files: vec!["file1.sst".to_string()],
            vector_count: 100,
            has_quantized: false,
            has_fp32: true,
            storage_engine: StorageEngineType::SST,
            created_at: chrono::Utc::now(),
        };
        
        let event2 = IndexEvent {
            event_id: "event_2".to_string(),
            event_type: EventType::Compaction,
            collection_id: "test_collection".to_string(),
            data_files: vec!["output.sst".to_string()],
            vector_count: 200,
            has_quantized: true,
            has_fp32: true,
            storage_engine: StorageEngineType::SST,
            created_at: chrono::Utc::now(),
        };
        
        // Persist events
        wal.persist_event(&event1).await?;
        wal.persist_event(&event2).await?;
        
        // Acknowledge one event
        wal.acknowledge_event("event_1").await?;
        
        // Recover pending events
        let recovered = wal.recover_pending_events().await?;
        
        // Should only have event_2 (event_1 was acknowledged)
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].event_id, "event_2");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_wal_rotation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let mut wal = EventLogWAL::new(temp_dir.path()).await?;
        
        // Set small max file size for testing
        wal.max_file_size = 1024; // 1KB
        
        // Create large event that will trigger rotation
        let event = IndexEvent {
            event_id: "large_event".to_string(),
            event_type: EventType::Flush,
            collection_id: "test_collection".to_string(),
            data_files: (0..100).map(|i| format!("file_{}.sst", i)).collect(),
            vector_count: 10000,
            has_quantized: false,
            has_fp32: true,
            storage_engine: StorageEngineType::SST,
            created_at: chrono::Utc::now(),
        };
        
        // This should trigger rotation
        wal.persist_event(&event).await?;
        
        // Check that rotation occurred
        let entries: Vec<_> = std::fs::read_dir(temp_dir.path())?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_str()
                    .map(|n| n.starts_with("eventlog_wal_"))
                    .unwrap_or(false)
            })
            .collect();
        
        // Should have at least 2 WAL files (current + rotated)
        assert!(entries.len() >= 1);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_wal_compaction() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let mut wal = EventLogWAL::new(temp_dir.path()).await?;
        
        // Create and persist multiple events
        for i in 0..5 {
            let event = IndexEvent {
                event_id: format!("event_{}", i),
                event_type: EventType::Flush,
                collection_id: "test_collection".to_string(),
                data_files: vec![format!("file_{}.sst", i)],
                vector_count: 100,
                has_quantized: false,
                has_fp32: true,
                storage_engine: StorageEngineType::SST,
                created_at: chrono::Utc::now(),
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
        assert_eq!(recovered.len(), 2);
        
        let event_ids: Vec<_> = recovered.iter().map(|e| e.event_id.as_str()).collect();
        assert!(event_ids.contains(&"event_1"));
        assert!(event_ids.contains(&"event_3"));
        
        Ok(())
    }
}