/*
 * Copyright 2025 ProximaDB
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

//! Global WAL Manifest for Multi-Disk Coordination
//!
//! This module implements a centralized manifest system that tracks WAL segments
//! across multiple storage disks, enabling cloud-optimized recovery and LSN tracking.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use once_cell::sync::OnceCell;

/// Status of a WAL entry in the manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WalEntryStatus {
    /// WAL segment is active and receiving writes
    Active,
    /// WAL segment has been flushed to storage
    Flushed,
    /// WAL segment has been compacted
    Compacted,
    /// WAL segment has been deleted
    Deleted,
}

/// Global manifest entry tracking a WAL segment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalManifestEntry {
    /// Global log sequence number
    pub global_lsn: u64,
    /// Batch ID
    pub batch_id: String,
    /// Collection UUID
    pub collection_id: String,
    /// Storage path (e.g., file:///tmp/proximadb1/data)
    pub storage_path: String,
    /// Relative path to WAL segment (also used as file_path)
    pub file_path: String,
    /// Current status
    pub status: WalEntryStatus,
    /// Serialization format
    pub format: String,
    /// CRC32 checksum
    pub checksum_crc32: u32,
    /// Creation timestamp
    pub created_at: i64,
    /// Size in bytes
    pub size_bytes: u64,
}

impl GlobalManifestEntry {
    /// Create a new manifest entry
    pub fn new(
        global_lsn: u64,
        batch_id: String,
        collection_id: String,
        storage_path: String,
        file_path: String,
        format: String,
        checksum_crc32: u32,
        size_bytes: u64,
    ) -> Self {
        Self {
            global_lsn,
            batch_id,
            collection_id,
            storage_path,
            file_path,
            status: WalEntryStatus::Active,
            format,
            checksum_crc32,
            created_at: chrono::Utc::now().timestamp(),
            size_bytes,
        }
    }

    /// Get the full URL to the WAL file
    pub fn full_url(&self) -> String {
        format!("{}/{}", self.storage_path.trim_end_matches('/'), self.file_path.trim_start_matches('/'))
    }
}

/// Checkpoint state for a collection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointCollectionState {
    /// Collection ID
    pub collection_id: String,
    /// Last flushed LSN
    pub last_flushed_lsn: u64,
    /// Timestamp of last checkpoint
    pub timestamp: i64,
}

/// Global checkpoint tracking all collections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalCheckpoint {
    /// Checkpoint ID
    pub checkpoint_id: u64,
    /// Global LSN at checkpoint
    pub global_lsn: u64,
    /// Per-collection state
    pub collections: HashMap<String, CheckpointCollectionState>,
    /// Timestamp
    pub timestamp: i64,
}

/// Global LSN allocator
pub struct GlobalLsnAllocator {
    next_lsn: Arc<RwLock<u64>>,
}

impl GlobalLsnAllocator {
    fn new(initial_lsn: u64) -> Self {
        Self {
            next_lsn: Arc::new(RwLock::new(initial_lsn)),
        }
    }

    /// Allocate next LSN
    pub async fn allocate(&self) -> u64 {
        let mut lsn = self.next_lsn.write().await;
        let allocated = *lsn;
        *lsn += 1;
        allocated
    }

    /// Get current LSN without allocating
    pub async fn current(&self) -> u64 {
        *self.next_lsn.read().await
    }
}

/// Configuration for Global Manifest Service
#[derive(Debug, Clone)]
pub struct GlobalManifestServiceConfig {
    /// URL for manifest storage
    pub manifest_url: String,
    /// Enable periodic checkpointing
    pub enable_checkpointing: bool,
    /// Checkpoint interval in seconds
    pub checkpoint_interval_secs: u64,
}

impl Default for GlobalManifestServiceConfig {
    fn default() -> Self {
        Self {
            manifest_url: "file:///tmp/proximadb/manifest".to_string(),
            enable_checkpointing: true,
            checkpoint_interval_secs: 300,
        }
    }
}

/// Global Manifest Service managing WAL across multiple disks
pub struct GlobalManifestService {
    config: GlobalManifestServiceConfig,
    entries: Arc<RwLock<Vec<GlobalManifestEntry>>>,
    lsn_allocator: GlobalLsnAllocator,
    checkpoints: Arc<RwLock<Vec<GlobalCheckpoint>>>,
}

impl GlobalManifestService {
    /// Create a new global manifest service
    pub fn new(config: GlobalManifestServiceConfig) -> Self {
        Self {
            config,
            entries: Arc::new(RwLock::new(Vec::new())),
            lsn_allocator: GlobalLsnAllocator::new(1),
            checkpoints: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a new WAL entry to the manifest
    pub async fn add_entry(&self, entry: GlobalManifestEntry) -> Result<()> {
        let mut entries = self.entries.write().await;
        entries.push(entry);
        Ok(())
    }

    /// Asynchronously append an entry to the manifest (alias for add_entry)
    pub async fn append_async(&self, entry: GlobalManifestEntry) -> Result<()> {
        self.add_entry(entry).await
    }

    /// Get all entries for a collection
    pub async fn get_entries_for_collection(&self, collection_id: &str) -> Vec<GlobalManifestEntry> {
        let entries = self.entries.read().await;
        entries
            .iter()
            .filter(|e| e.collection_id == collection_id)
            .cloned()
            .collect()
    }

    /// Update entry status
    pub async fn update_entry_status(&self, lsn: u64, new_status: WalEntryStatus) -> Result<()> {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.iter_mut().find(|e| e.global_lsn == lsn) {
            entry.status = new_status;
        }
        Ok(())
    }

    /// Allocate a new LSN
    pub async fn allocate_lsn(&self) -> u64 {
        self.lsn_allocator.allocate().await
    }

    /// Get current LSN
    pub async fn current_lsn(&self) -> u64 {
        self.lsn_allocator.current().await
    }

    /// Create a checkpoint
    pub async fn create_checkpoint(&self) -> Result<GlobalCheckpoint> {
        let entries = self.entries.read().await;
        let current_lsn = self.lsn_allocator.current().await;

        let mut collections = HashMap::new();
        for entry in entries.iter() {
            if entry.status == WalEntryStatus::Flushed || entry.status == WalEntryStatus::Compacted {
                collections
                    .entry(entry.collection_id.clone())
                    .and_modify(|state: &mut CheckpointCollectionState| {
                        if entry.global_lsn > state.last_flushed_lsn {
                            state.last_flushed_lsn = entry.global_lsn;
                        }
                    })
                    .or_insert(CheckpointCollectionState {
                        collection_id: entry.collection_id.clone(),
                        last_flushed_lsn: entry.global_lsn,
                        timestamp: chrono::Utc::now().timestamp(),
                    });
            }
        }

        let checkpoint = GlobalCheckpoint {
            checkpoint_id: current_lsn,
            global_lsn: current_lsn,
            collections,
            timestamp: chrono::Utc::now().timestamp(),
        };

        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.push(checkpoint.clone());

        Ok(checkpoint)
    }

    /// Load manifest from storage
    pub async fn load(&self) -> Result<()> {
        // TODO: Implement manifest loading from filesystem
        tracing::info!("Global manifest load not yet implemented");
        Ok(())
    }

    /// Save manifest to storage
    pub async fn save(&self) -> Result<()> {
        // TODO: Implement manifest saving to filesystem
        tracing::info!("Global manifest save not yet implemented");
        Ok(())
    }
}

// Singleton instance
static GLOBAL_MANIFEST: OnceCell<Arc<GlobalManifestService>> = OnceCell::new();

/// Initialize the global manifest service
pub fn init(config: GlobalManifestServiceConfig) -> Result<Arc<GlobalManifestService>> {
    let service = Arc::new(GlobalManifestService::new(config));
    GLOBAL_MANIFEST
        .set(service.clone())
        .map_err(|_| anyhow::anyhow!("Global manifest already initialized"))?;
    Ok(service)
}

/// Get the global manifest service instance
pub fn get_service() -> Option<Arc<GlobalManifestService>> {
    GLOBAL_MANIFEST.get().cloned()
}

/// Shutdown the global manifest service
pub async fn shutdown() -> Result<()> {
    if let Some(service) = GLOBAL_MANIFEST.get() {
        service.save().await?;
    }
    Ok(())
}

/// Get all active entries from the global manifest
pub async fn get_active_entries() -> Vec<GlobalManifestEntry> {
    if let Some(service) = GLOBAL_MANIFEST.get() {
        let entries = service.entries.read().await;
        entries
            .iter()
            .filter(|e| e.status == WalEntryStatus::Active)
            .cloned()
            .collect()
    } else {
        Vec::new()
    }
}

/// Get all entries for a specific collection
pub async fn get_collection_entries(collection_id: &str) -> Vec<GlobalManifestEntry> {
    if let Some(service) = GLOBAL_MANIFEST.get() {
        service.get_entries_for_collection(collection_id).await
    } else {
        Vec::new()
    }
}

/// Mark batch IDs as flushed in the manifest
pub async fn mark_flushed(batch_ids: &[String]) -> Result<()> {
    if let Some(service) = GLOBAL_MANIFEST.get() {
        let mut entries = service.entries.write().await;
        for entry in entries.iter_mut() {
            // Match by WAL segment path containing the batch ID
            for batch_id in batch_ids {
                if entry.file_path.contains(batch_id) && entry.status == WalEntryStatus::Active {
                    entry.status = WalEntryStatus::Flushed;
                }
            }
        }
    }
    Ok(())
}

/// Cleanup checkpointed/flushed entries from the manifest
pub async fn cleanup_checkpointed() -> Result<usize> {
    if let Some(service) = GLOBAL_MANIFEST.get() {
        let mut entries = service.entries.write().await;
        let before_count = entries.len();
        entries.retain(|e| e.status != WalEntryStatus::Flushed && e.status != WalEntryStatus::Compacted);
        let removed = before_count - entries.len();
        Ok(removed)
    } else {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lsn_allocation() {
        let allocator = GlobalLsnAllocator::new(1);
        let lsn1 = allocator.allocate().await;
        let lsn2 = allocator.allocate().await;
        assert_eq!(lsn1, 1);
        assert_eq!(lsn2, 2);
    }

    #[tokio::test]
    async fn test_manifest_entry_management() {
        let config = GlobalManifestServiceConfig::default();
        let service = GlobalManifestService::new(config);

        let entry = GlobalManifestEntry::new(
            service.allocate_lsn().await,
            "batch_001".to_string(),
            "test_collection".to_string(),
            "file:///tmp/test".to_string(),
            "wal/segment_001.wal".to_string(),
            "proto".to_string(),
            12345,
            1024,
        );

        service.add_entry(entry.clone()).await.unwrap();

        let entries = service.get_entries_for_collection("test_collection").await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].collection_id, "test_collection");
    }

    #[tokio::test]
    async fn test_checkpoint_creation() {
        let config = GlobalManifestServiceConfig::default();
        let service = GlobalManifestService::new(config);

        // Add some entries
        for i in 0..3 {
            let mut entry = GlobalManifestEntry::new(
                service.allocate_lsn().await,
                format!("batch_{:03}", i),
                format!("collection_{}", i % 2),
                "file:///tmp/test".to_string(),
                format!("wal/segment_{:03}.wal", i),
                "proto".to_string(),
                12345 + i as u32,
                1024,
            );
            entry.status = WalEntryStatus::Flushed;
            service.add_entry(entry).await.unwrap();
        }

        let checkpoint = service.create_checkpoint().await.unwrap();
        assert!(checkpoint.collections.len() >= 1);
    }
}
