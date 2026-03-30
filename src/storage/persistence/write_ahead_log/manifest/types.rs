#![allow(dead_code)]
//! Global WAL Manifest
//!
//! Provides a centralized manifest for tracking WAL files across all collections.
//! This enables:
//! - Global recovery ordering via monotonic LSN
//! - Cross-collection consistency
//! - Efficient checkpoint management
//! - Simplified disaster recovery
//!
//! Architecture:
//! ```text
//! Multi-disk configuration with centralized global manifest:
//!
//! DISK 1 (Primary - hosts global manifest):
//! /tmp/proximadb1/data/
//!   ├── wal/
//!   │   ├── global_manifest.log      # ✨ GLOBAL: Tracks ALL collections
//!   │   └── checkpoint.state         # ✨ GLOBAL: Latest checkpoint
//!   ├── {collection_A}/
//!   │   ├── wal/{batch_id}.bcwal     # Collection A WAL (if assigned here)
//!   │   └── data/*.sst               # Collection A data
//!   └── ...
//!
//! DISK 2 (Secondary - collection WAL files only):
//! /tmp/proximadb2/data/
//!   ├── {collection_B}/
//!   │   ├── wal/{batch_id}.bcwal     # Collection B WAL (if assigned here)
//!   │   └── data/*.parquet           # Collection B data
//!   └── ...
//!
//! The global manifest at DISK 1 tracks WAL files on ALL disks.
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::storage::persistence::write_ahead_log::{BatchId, serialization::SerializationFormat};

/// Status of a WAL entry in the global manifest
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WalEntryStatus {
    /// Active WAL file, not yet flushed to storage engine
    Active,
    /// Flushed to storage engine, safe to archive
    Flushed,
    /// Archived to long-term storage, can be deleted
    Archived,
    /// Rolled back during PITR recovery (not to be recovered)
    RolledBack,
}

/// Global manifest entry tracking a single WAL batch across all collections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalManifestEntry {
    /// Global LSN (monotonically increasing across all collections)
    pub global_lsn: u64,

    /// Collection identifier
    pub collection_id: String,

    /// Batch identifier (base62 encoded)
    pub batch_id: String,

    /// Relative path from base_url: {collection_id}/wal/{filename}
    /// Example: "1v5XVSY/wal/8WBT...bcwal"
    /// This path is relative to the storage location where the collection is assigned
    pub file_path: String,

    /// Storage location URL where this WAL file resides
    /// Example: "file:///tmp/proximadb1/data" or "file:///tmp/proximadb2/data"
    /// This allows the manifest to track files across multiple disks
    pub storage_url: String,

    /// File size in bytes
    pub size_bytes: u64,

    /// CRC32 checksum
    pub checksum_crc32: u32,

    /// Timestamp when batch was created (milliseconds since epoch)
    pub timestamp_ms: u64,

    /// Serialization format (bincode, avro, proto)
    pub format: SerializationFormat,

    /// Number of vectors in this batch
    pub vector_count: u64,

    /// Status: Active, Flushed, Archived
    pub status: WalEntryStatus,

    /// Optional: checkpoint ID if this batch is part of a checkpoint
    pub checkpoint_id: Option<u64>,
}

impl GlobalManifestEntry {
    /// Create a new active manifest entry
    pub fn new(
        global_lsn: u64,
        collection_id: String,
        batch_id: &BatchId,
        file_name: String,
        size_bytes: u64,
        checksum_crc32: u32,
        format: SerializationFormat,
        vector_count: u64,
        storage_url: String, // Which disk this WAL file is on
    ) -> Self {
        let timestamp_ms = batch_id.timestamp_ms();
        // Keep existing structure: {collection_id}/wal/{file_name}
        let file_path = format!("{}/wal/{}", collection_id, file_name);

        Self {
            global_lsn,
            collection_id,
            batch_id: batch_id.to_base62(),
            file_path,
            storage_url,
            size_bytes,
            checksum_crc32,
            timestamp_ms,
            format,
            vector_count,
            status: WalEntryStatus::Active,
            checkpoint_id: None,
        }
    }

    /// Get the full URL to this WAL file
    pub fn full_url(&self) -> String {
        format!("{}/{}", self.storage_url, self.file_path)
    }
}

/// Global checkpoint state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalCheckpoint {
    /// Checkpoint identifier (monotonically increasing)
    pub checkpoint_id: u64,

    /// Global LSN at checkpoint time
    pub checkpoint_lsn: u64,

    /// Timestamp of checkpoint
    pub timestamp_ms: u64,

    /// Collections included in this checkpoint
    pub collections: Vec<CheckpointCollectionState>,

    /// All WAL entries with global_lsn < safe_to_delete_before_lsn can be safely deleted
    pub safe_to_delete_before_lsn: u64,
}

/// Per-collection state at checkpoint time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointCollectionState {
    pub collection_id: String,
    pub last_flushed_lsn: u64,
    pub vector_count: u64,
}

/// Global manifest manager
#[allow(dead_code)]
pub struct GlobalManifest {
    /// Filesystem factory for I/O
    filesystem_factory: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,

    /// Base WAL directory (e.g., "file:///tmp/proximadb2/wal")
    wal_base_url: String,

    /// In-memory cache of manifest entries (sorted by global_lsn)
    entries: Arc<RwLock<Vec<GlobalManifestEntry>>>,

    /// Latest checkpoint
    latest_checkpoint: Arc<RwLock<Option<GlobalCheckpoint>>>,

    /// Global LSN allocator
    lsn_allocator: Arc<GlobalLsnAllocator>,
}

/// Global LSN allocator - ensures monotonically increasing LSN across all collections
pub struct GlobalLsnAllocator {
    next_lsn: Arc<RwLock<u64>>,
}

impl GlobalLsnAllocator {
    /// Create a new LSN allocator starting from a given LSN
    pub fn new(start_lsn: u64) -> Self {
        Self {
            next_lsn: Arc::new(RwLock::new(start_lsn)),
        }
    }

    /// Allocate the next global LSN
    pub async fn allocate(&self) -> u64 {
        let mut lsn = self.next_lsn.write().await;
        let current = *lsn;
        *lsn += 1;
        current
    }

    /// Get the current LSN without allocating
    pub async fn current(&self) -> u64 {
        *self.next_lsn.read().await
    }

    /// Set the next LSN (used during recovery)
    pub async fn set_next(&self, next_lsn: u64) {
        let mut lsn = self.next_lsn.write().await;
        *lsn = next_lsn;
    }
}

impl GlobalManifest {
    /// Create a new global manifest
    pub async fn new(
        filesystem_factory: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
        wal_base_url: String,
    ) -> Result<Self> {
        info!("🌐 Creating GlobalManifest at {}", wal_base_url);

        let manifest = Self {
            filesystem_factory,
            wal_base_url,
            entries: Arc::new(RwLock::new(Vec::new())),
            latest_checkpoint: Arc::new(RwLock::new(None)),
            lsn_allocator: Arc::new(GlobalLsnAllocator::new(1)),
        };

        // Load existing manifest if it exists
        manifest.load_from_disk().await?;

        Ok(manifest)
    }

    /// Get the global manifest URL
    fn global_manifest_url(&self) -> String {
        format!("{}/global_manifest.log", self.wal_base_url)
    }

    /// Get the checkpoint state URL
    fn checkpoint_url(&self) -> String {
        format!("{}/checkpoint.state", self.wal_base_url)
    }

    /// Load manifest from disk
    async fn load_from_disk(&self) -> Result<()> {
        let url = self.global_manifest_url();
        let fs = self.filesystem_factory.get_filesystem(&url)?;

        if !fs.exists(&url).await? {
            info!("📝 No existing global manifest found, starting fresh");
            return Ok(());
        }

        let data = fs
            .read(&url)
            .await
            .context("Failed to read global manifest")?;

        let mut entries = Vec::new();
        let mut max_lsn = 0u64;

        for line in data.split(|b| *b == b'\n') {
            if line.is_empty() {
                continue;
            }

            match serde_json::from_slice::<GlobalManifestEntry>(line) {
                Ok(entry) => {
                    max_lsn = max_lsn.max(entry.global_lsn);
                    entries.push(entry);
                }
                Err(e) => {
                    warn!("⚠️  Failed to parse manifest entry: {}", e);
                }
            }
        }

        // Sort by global LSN
        entries.sort_by_key(|e| e.global_lsn);

        *self.entries.write().await = entries;

        // Set next LSN to be one more than the maximum
        self.lsn_allocator.set_next(max_lsn + 1).await;

        info!(
            "✅ Loaded {} manifest entries, next LSN: {}",
            self.entries.read().await.len(),
            self.lsn_allocator.current().await
        );

        // Load checkpoint if it exists
        self.load_checkpoint().await?;

        Ok(())
    }

    /// Load checkpoint from disk
    async fn load_checkpoint(&self) -> Result<()> {
        let url = self.checkpoint_url();
        let fs = self.filesystem_factory.get_filesystem(&url)?;

        if !fs.exists(&url).await? {
            debug!("No existing checkpoint found");
            return Ok(());
        }

        let data = fs.read(&url).await.context("Failed to read checkpoint")?;

        let checkpoint: GlobalCheckpoint =
            serde_json::from_slice(&data).context("Failed to parse checkpoint")?;

        *self.latest_checkpoint.write().await = Some(checkpoint.clone());

        info!(
            "✅ Loaded checkpoint {} at LSN {}",
            checkpoint.checkpoint_id, checkpoint.checkpoint_lsn
        );

        Ok(())
    }

    /// Append a new entry to the global manifest
    pub async fn append_entry(&self, mut entry: GlobalManifestEntry) -> Result<()> {
        // Allocate global LSN if not set
        if entry.global_lsn == 0 {
            entry.global_lsn = self.lsn_allocator.allocate().await;
        }

        debug!(
            "📝 Appending manifest entry: LSN={}, collection={}, batch={}",
            entry.global_lsn, entry.collection_id, entry.batch_id
        );

        // Add to in-memory cache
        let mut entries = self.entries.write().await;
        entries.push(entry.clone());
        entries.sort_by_key(|e| e.global_lsn);
        drop(entries);

        // Append to disk (atomic)
        self.write_entry_to_disk(&entry).await?;

        Ok(())
    }

    /// Write a single entry to disk (append-only)
    async fn write_entry_to_disk(&self, entry: &GlobalManifestEntry) -> Result<()> {
        let url = self.global_manifest_url();
        let fs = self.filesystem_factory.get_filesystem(&url)?;

        // Read current content
        let mut content = if fs.exists(&url).await? {
            fs.read(&url).await?
        } else {
            Vec::new()
        };

        // Append new entry as JSON line
        let mut line = serde_json::to_vec(entry).context("Failed to serialize manifest entry")?;
        line.push(b'\n');
        content.extend_from_slice(&line);

        // Write atomically
        let strategy = crate::storage::persistence::filesystem::write_strategy::WriteStrategyFactory
            ::create_metadata_strategy(&*fs, None)?;
        let opts = strategy.create_file_options(&*fs, &url)?;
        fs.write(&url, &content, Some(opts))
            .await
            .context("Failed to write global manifest")?;

        // Best-effort sync
        let _ = fs.sync_file(&url).await;

        Ok(())
    }

    /// Get all entries for a specific collection
    pub async fn get_collection_entries(&self, collection_id: &str) -> Vec<GlobalManifestEntry> {
        let entries = self.entries.read().await;
        entries
            .iter()
            .filter(|e| e.collection_id == collection_id)
            .cloned()
            .collect()
    }

    /// Get all active entries (not flushed or archived)
    pub async fn get_active_entries(&self) -> Vec<GlobalManifestEntry> {
        let entries = self.entries.read().await;
        entries
            .iter()
            .filter(|e| e.status == WalEntryStatus::Active)
            .cloned()
            .collect()
    }

    /// Get all entries sorted by global LSN
    pub async fn get_all_entries(&self) -> Vec<GlobalManifestEntry> {
        self.entries.read().await.clone()
    }

    /// Mark entries as flushed
    pub async fn mark_flushed(&self, batch_ids: &[String]) -> Result<()> {
        let mut entries = self.entries.write().await;
        let mut modified = false;

        for entry in entries.iter_mut() {
            if batch_ids.contains(&entry.batch_id) && entry.status == WalEntryStatus::Active {
                entry.status = WalEntryStatus::Flushed;
                modified = true;
            }
        }

        drop(entries);

        if modified {
            self.rewrite_manifest().await?;
        }

        Ok(())
    }

    /// Rewrite the entire manifest (used after status updates)
    async fn rewrite_manifest(&self) -> Result<()> {
        let entries = self.entries.read().await;

        let url = self.global_manifest_url();
        let fs = self.filesystem_factory.get_filesystem(&url)?;

        let mut buf = Vec::new();
        for entry in entries.iter() {
            let mut line =
                serde_json::to_vec(entry).context("Failed to serialize manifest entry")?;
            line.push(b'\n');
            buf.extend_from_slice(&line);
        }

        let strategy = crate::storage::persistence::filesystem::write_strategy::WriteStrategyFactory
            ::create_metadata_strategy(&*fs, None)?;
        let opts = strategy.create_file_options(&*fs, &url)?;
        fs.write(&url, &buf, Some(opts))
            .await
            .context("Failed to rewrite global manifest")?;

        let _ = fs.sync_file(&url).await;

        info!("✅ Rewrote global manifest with {} entries", entries.len());

        Ok(())
    }

    /// Create a new checkpoint
    pub async fn create_checkpoint(&self) -> Result<GlobalCheckpoint> {
        let entries = self.entries.read().await;

        // Get the latest checkpoint ID
        let checkpoint_id = {
            let latest = self.latest_checkpoint.read().await;
            latest.as_ref().map_or(1, |c| c.checkpoint_id + 1)
        };

        // Find the highest flushed LSN
        let checkpoint_lsn = entries
            .iter()
            .filter(|e| e.status == WalEntryStatus::Flushed)
            .map(|e| e.global_lsn)
            .max()
            .unwrap_or(0);

        // Group by collection
        let mut collection_map: HashMap<String, CheckpointCollectionState> = HashMap::new();
        for entry in entries.iter() {
            if entry.status == WalEntryStatus::Flushed && entry.global_lsn <= checkpoint_lsn {
                collection_map
                    .entry(entry.collection_id.clone())
                    .and_modify(|state| {
                        state.last_flushed_lsn = state.last_flushed_lsn.max(entry.global_lsn);
                        state.vector_count += entry.vector_count;
                    })
                    .or_insert(CheckpointCollectionState {
                        collection_id: entry.collection_id.clone(),
                        last_flushed_lsn: entry.global_lsn,
                        vector_count: entry.vector_count,
                    });
            }
        }

        let checkpoint = GlobalCheckpoint {
            checkpoint_id,
            checkpoint_lsn,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
            collections: collection_map.into_values().collect(),
            safe_to_delete_before_lsn: checkpoint_lsn,
        };

        drop(entries);

        // Save checkpoint to disk
        self.save_checkpoint(&checkpoint).await?;

        // Update in-memory checkpoint
        *self.latest_checkpoint.write().await = Some(checkpoint.clone());

        info!(
            "✅ Created checkpoint {} at LSN {}",
            checkpoint.checkpoint_id, checkpoint.checkpoint_lsn
        );

        Ok(checkpoint)
    }

    /// Save checkpoint to disk
    async fn save_checkpoint(&self, checkpoint: &GlobalCheckpoint) -> Result<()> {
        let url = self.checkpoint_url();
        let fs = self.filesystem_factory.get_filesystem(&url)?;

        let data =
            serde_json::to_vec_pretty(checkpoint).context("Failed to serialize checkpoint")?;

        let strategy = crate::storage::persistence::filesystem::write_strategy::WriteStrategyFactory
            ::create_metadata_strategy(&*fs, None)?;
        let opts = strategy.create_file_options(&*fs, &url)?;
        fs.write(&url, &data, Some(opts))
            .await
            .context("Failed to write checkpoint")?;

        let _ = fs.sync_file(&url).await;

        Ok(())
    }

    /// Get the latest checkpoint
    pub async fn get_latest_checkpoint(&self) -> Option<GlobalCheckpoint> {
        self.latest_checkpoint.read().await.clone()
    }

    /// Clean up old WAL entries that have been checkpointed
    pub async fn cleanup_checkpointed_entries(&self) -> Result<usize> {
        let checkpoint = match self.latest_checkpoint.read().await.clone() {
            Some(cp) => cp,
            None => {
                debug!("No checkpoint exists, skipping cleanup");
                return Ok(0);
            }
        };

        let mut entries = self.entries.write().await;
        let original_count = entries.len();

        // Remove entries that are checkpointed and can be safely deleted
        entries.retain(|e| {
            e.global_lsn >= checkpoint.safe_to_delete_before_lsn
                || e.status == WalEntryStatus::Active
        });

        let removed_count = original_count - entries.len();
        drop(entries);

        if removed_count > 0 {
            self.rewrite_manifest().await?;
            info!("🧹 Cleaned up {} checkpointed WAL entries", removed_count);
        }

        Ok(removed_count)
    }

    /// Get the global LSN allocator
    pub fn lsn_allocator(&self) -> Arc<GlobalLsnAllocator> {
        self.lsn_allocator.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lsn_allocator() {
        let allocator = GlobalLsnAllocator::new(100);

        assert_eq!(allocator.allocate().await, 100);
        assert_eq!(allocator.allocate().await, 101);
        assert_eq!(allocator.allocate().await, 102);
        assert_eq!(allocator.current().await, 103);
    }

    #[tokio::test]
    async fn test_manifest_entry_creation() {
        let batch_id = BatchId::new();
        let entry = GlobalManifestEntry::new(
            1,
            "test_collection".to_string(),
            &batch_id,
            "test.bcwal".to_string(),
            1024,
            12345,
            SerializationFormat::Bincode,
            100,
            "file:///tmp/proximadb1/data".to_string(),
        );

        assert_eq!(entry.global_lsn, 1);
        assert_eq!(entry.collection_id, "test_collection");
        assert_eq!(entry.size_bytes, 1024);
        assert_eq!(entry.vector_count, 100);
        assert_eq!(entry.status, WalEntryStatus::Active);
        assert_eq!(entry.file_path, "test_collection/wal/test.bcwal");
        assert_eq!(entry.storage_url, "file:///tmp/proximadb1/data");
        assert_eq!(
            entry.full_url(),
            "file:///tmp/proximadb1/data/test_collection/wal/test.bcwal"
        );
    }
}
