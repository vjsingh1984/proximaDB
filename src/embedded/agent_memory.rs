//! Agent Memory Management with Incremental Checkpoints
//!
//! This module provides checkpoint and delta save/load functionality for
//! embedded mode, enabling efficient persistence for agentic AI applications.
//!
//! # Features
//!
//! - **Named Checkpoints**: Create named snapshots of database state
//! - **Delta Saves**: Persist only changes since the last checkpoint
//! - **Efficient Recovery**: Restore to any named checkpoint quickly
//! - **Thread-Safe**: Safe for concurrent access from multiple agents
//!
//! # Usage
//!
//! ```ignore
//! let db = EmbeddedProximaDB::new(config)?;
//!
//! // Create a named checkpoint
//! let info = db.checkpoint("before_experiment")?;
//! println!("Checkpoint created: {} bytes", info.size_bytes);
//!
//! // Make changes...
//! db.insert("vectors", ids, vectors, None)?;
//!
//! // Save only the delta (changes since checkpoint)
//! let delta = db.save_delta("/tmp/delta_001.delta")?;
//!
//! // Or restore to the checkpoint
//! db.restore_checkpoint("before_experiment")?;
//! ```

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Information about a created checkpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInfo {
    /// Name of the checkpoint
    pub name: String,
    /// Timestamp when checkpoint was created
    pub timestamp: DateTime<Utc>,
    /// Total size of the checkpoint in bytes
    pub size_bytes: u64,
    /// Collections included in this checkpoint
    pub collections: Vec<String>,
    /// Global LSN at checkpoint time
    pub checkpoint_lsn: u64,
    /// Per-collection state at checkpoint
    pub collection_states: HashMap<String, CollectionCheckpointState>,
}

/// Per-collection state at checkpoint time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionCheckpointState {
    /// Collection name
    pub name: String,
    /// Number of vectors at checkpoint time
    pub vector_count: u64,
    /// LSN of the last entry for this collection
    pub last_lsn: u64,
    /// Dimension of vectors in this collection
    pub dimension: u32,
    /// Storage engine type
    pub engine: String,
}

/// Information about an incremental delta save
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaInfo {
    /// Path where delta was saved
    pub path: String,
    /// Timestamp when delta was created
    pub timestamp: DateTime<Utc>,
    /// Size of the delta file in bytes
    pub size_bytes: u64,
    /// Number of entries in the delta
    pub entry_count: u64,
    /// Base checkpoint name (if any)
    pub base_checkpoint: Option<String>,
    /// Starting LSN of the delta
    pub start_lsn: u64,
    /// Ending LSN of the delta (inclusive)
    pub end_lsn: u64,
    /// Collections with changes in this delta
    pub affected_collections: Vec<String>,
}

/// Delta file header for serialization
/// Internal header for delta files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaHeader {
    /// Magic number for file identification
    magic: [u8; 4],
    /// Version of the delta format
    version: u32,
    /// Timestamp when delta was created
    pub timestamp_ms: i64,
    /// Base checkpoint name (if any)
    pub base_checkpoint: Option<String>,
    /// Starting LSN
    pub start_lsn: u64,
    /// Ending LSN
    pub end_lsn: u64,
    /// Number of entries
    pub entry_count: u64,
    /// CRC32 checksum of the data section
    data_checksum: u32,
}

impl DeltaHeader {
    const MAGIC: [u8; 4] = *b"PDEL"; // ProximaDB Delta
    const VERSION: u32 = 1;

    fn new(
        base_checkpoint: Option<String>,
        start_lsn: u64,
        end_lsn: u64,
        entry_count: u64,
    ) -> Self {
        Self {
            magic: Self::MAGIC,
            version: Self::VERSION,
            timestamp_ms: Utc::now().timestamp_millis(),
            base_checkpoint,
            start_lsn,
            end_lsn,
            entry_count,
            data_checksum: 0, // Set after data is written
        }
    }
}

/// Delta entry representing a single change
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaEntry {
    /// Global LSN of this entry
    pub lsn: u64,
    /// Collection ID
    pub collection_id: String,
    /// Operation type: "upsert", "delete", "collection_create", "collection_delete"
    pub operation: String,
    /// Serialized vector records (for upsert operations)
    pub vector_data: Option<Vec<u8>>,
    /// Vector IDs (for delete operations)
    pub vector_ids: Option<Vec<String>>,
    /// Collection config (for collection operations)
    pub collection_config: Option<Vec<u8>>,
}

/// Checkpoint manager for the embedded database
pub struct CheckpointManager {
    /// Base directory for checkpoint storage
    base_path: PathBuf,
    /// Named checkpoints (name -> CheckpointInfo)
    checkpoints: Arc<RwLock<HashMap<String, CheckpointInfo>>>,
    /// Current checkpoint name (if any)
    current_checkpoint: Arc<RwLock<Option<String>>>,
    /// LSN at last checkpoint (or 0 if none)
    last_checkpoint_lsn: Arc<RwLock<u64>>,
}

impl CheckpointManager {
    /// Create a new checkpoint manager
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        let base_path = base_path.into();
        Self {
            base_path,
            checkpoints: Arc::new(RwLock::new(HashMap::new())),
            current_checkpoint: Arc::new(RwLock::new(None)),
            last_checkpoint_lsn: Arc::new(RwLock::new(0)),
        }
    }

    /// Initialize the checkpoint manager and load existing checkpoints
    pub async fn init(&self) -> Result<()> {
        // Create checkpoint directory if it doesn't exist
        let checkpoint_dir = self.checkpoint_dir();
        tokio::fs::create_dir_all(&checkpoint_dir)
            .await
            .context("Failed to create checkpoint directory")?;

        // Load existing checkpoints from disk
        self.load_checkpoints().await?;

        info!(
            "Checkpoint manager initialized at {:?}, found {} checkpoints",
            checkpoint_dir,
            self.checkpoints.read().await.len()
        );

        Ok(())
    }

    /// Get the checkpoint directory
    fn checkpoint_dir(&self) -> PathBuf {
        self.base_path.join("checkpoints")
    }

    /// Get the path for a specific checkpoint
    fn checkpoint_path(&self, name: &str) -> PathBuf {
        self.checkpoint_dir().join(format!("{}.checkpoint", name))
    }

    /// Load existing checkpoints from disk
    async fn load_checkpoints(&self) -> Result<()> {
        let checkpoint_dir = self.checkpoint_dir();
        if !checkpoint_dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&checkpoint_dir)
            .await
            .context("Failed to read checkpoint directory")?;

        let mut checkpoints = self.checkpoints.write().await;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "checkpoint") {
                match self.load_checkpoint_file(&path).await {
                    Ok(info) => {
                        debug!("Loaded checkpoint: {}", info.name);
                        checkpoints.insert(info.name.clone(), info);
                    }
                    Err(e) => {
                        warn!("Failed to load checkpoint {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Load a single checkpoint file
    async fn load_checkpoint_file(&self, path: &Path) -> Result<CheckpointInfo> {
        let data = tokio::fs::read(path)
            .await
            .context("Failed to read checkpoint file")?;
        let info: CheckpointInfo =
            bincode::deserialize(&data).context("Failed to deserialize checkpoint")?;
        Ok(info)
    }

    /// Save a checkpoint to disk
    async fn save_checkpoint(&self, info: &CheckpointInfo) -> Result<()> {
        let path = self.checkpoint_path(&info.name);
        let data = bincode::serialize(info).context("Failed to serialize checkpoint")?;
        tokio::fs::write(&path, &data)
            .await
            .context("Failed to write checkpoint file")?;
        debug!("Saved checkpoint {} to {:?}", info.name, path);
        Ok(())
    }

    /// Create a new checkpoint
    pub async fn create_checkpoint(
        &self,
        name: &str,
        current_lsn: u64,
        collections: Vec<CollectionCheckpointState>,
    ) -> Result<CheckpointInfo> {
        // Calculate total size
        let total_size: u64 = collections.iter().map(|c| c.vector_count * 4096).sum(); // Rough estimate

        let collection_states: HashMap<String, CollectionCheckpointState> = collections
            .iter()
            .map(|c| (c.name.clone(), c.clone()))
            .collect();

        let info = CheckpointInfo {
            name: name.to_string(),
            timestamp: Utc::now(),
            size_bytes: total_size,
            collections: collections.iter().map(|c| c.name.clone()).collect(),
            checkpoint_lsn: current_lsn,
            collection_states,
        };

        // Save to disk
        self.save_checkpoint(&info).await?;

        // Update in-memory state
        {
            let mut checkpoints = self.checkpoints.write().await;
            checkpoints.insert(name.to_string(), info.clone());
        }
        {
            let mut current = self.current_checkpoint.write().await;
            *current = Some(name.to_string());
        }
        {
            let mut lsn = self.last_checkpoint_lsn.write().await;
            *lsn = current_lsn;
        }

        info!(
            "Created checkpoint '{}' at LSN {} with {} collections",
            name,
            current_lsn,
            info.collections.len()
        );

        Ok(info)
    }

    /// Get checkpoint info by name
    pub async fn get_checkpoint(&self, name: &str) -> Option<CheckpointInfo> {
        self.checkpoints.read().await.get(name).cloned()
    }

    /// List all checkpoints
    pub async fn list_checkpoints(&self) -> Vec<CheckpointInfo> {
        let checkpoints = self.checkpoints.read().await;
        let mut list: Vec<_> = checkpoints.values().cloned().collect();
        list.sort_by_key(|c| c.timestamp);
        list
    }

    /// Delete a checkpoint
    pub async fn delete_checkpoint(&self, name: &str) -> Result<bool> {
        let path = self.checkpoint_path(name);

        // Remove from in-memory state
        let existed = {
            let mut checkpoints = self.checkpoints.write().await;
            checkpoints.remove(name).is_some()
        };

        // Delete file if it exists
        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .context("Failed to delete checkpoint file")?;
        }

        Ok(existed)
    }

    /// Get the last checkpoint LSN
    pub async fn last_checkpoint_lsn(&self) -> u64 {
        *self.last_checkpoint_lsn.read().await
    }

    /// Get the current checkpoint name
    pub async fn current_checkpoint_name(&self) -> Option<String> {
        self.current_checkpoint.read().await.clone()
    }

    /// Save a delta file
    pub async fn save_delta(
        &self,
        path: &str,
        entries: Vec<DeltaEntry>,
        base_checkpoint: Option<String>,
        start_lsn: u64,
        end_lsn: u64,
    ) -> Result<DeltaInfo> {
        // Serialize entries
        let entries_data =
            bincode::serialize(&entries).context("Failed to serialize delta entries")?;

        // Calculate checksum
        let checksum = crc32fast::hash(&entries_data);

        // Create header
        let mut header = DeltaHeader::new(
            base_checkpoint.clone(),
            start_lsn,
            end_lsn,
            entries.len() as u64,
        );
        header.data_checksum = checksum;

        // Serialize header
        let header_data =
            bincode::serialize(&header).context("Failed to serialize delta header")?;
        let header_len = header_data.len() as u32;

        // Write file: [header_len: u32][header][entries]
        let mut file_data = Vec::with_capacity(4 + header_data.len() + entries_data.len());
        file_data.extend_from_slice(&header_len.to_le_bytes());
        file_data.extend_from_slice(&header_data);
        file_data.extend_from_slice(&entries_data);

        // Write to file
        tokio::fs::write(path, &file_data)
            .await
            .context("Failed to write delta file")?;

        // Collect affected collections
        let affected_collections: Vec<String> = entries
            .iter()
            .map(|e| e.collection_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let info = DeltaInfo {
            path: path.to_string(),
            timestamp: Utc::now(),
            size_bytes: file_data.len() as u64,
            entry_count: entries.len() as u64,
            base_checkpoint,
            start_lsn,
            end_lsn,
            affected_collections,
        };

        info!(
            "Saved delta to {} with {} entries ({} bytes)",
            path,
            entries.len(),
            file_data.len()
        );

        Ok(info)
    }

    /// Load a delta file
    pub async fn load_delta(&self, path: &str) -> Result<(DeltaHeader, Vec<DeltaEntry>)> {
        // Read file
        let file_data = tokio::fs::read(path)
            .await
            .context("Failed to read delta file")?;

        if file_data.len() < 4 {
            anyhow::bail!("Delta file too small");
        }

        // Read header length
        let header_len =
            u32::from_le_bytes([file_data[0], file_data[1], file_data[2], file_data[3]]) as usize;

        if file_data.len() < 4 + header_len {
            anyhow::bail!("Delta file truncated");
        }

        // Deserialize header
        let header: DeltaHeader = bincode::deserialize(&file_data[4..4 + header_len])
            .context("Failed to deserialize delta header")?;

        // Verify magic
        if header.magic != DeltaHeader::MAGIC {
            anyhow::bail!("Invalid delta file magic");
        }

        // Deserialize entries
        let entries_data = &file_data[4 + header_len..];
        let entries: Vec<DeltaEntry> =
            bincode::deserialize(entries_data).context("Failed to deserialize delta entries")?;

        // Verify checksum
        let checksum = crc32fast::hash(entries_data);
        if checksum != header.data_checksum {
            anyhow::bail!(
                "Delta file checksum mismatch: expected {}, got {}",
                header.data_checksum,
                checksum
            );
        }

        info!(
            "Loaded delta from {} with {} entries (LSN {}..{})",
            path,
            entries.len(),
            header.start_lsn,
            header.end_lsn
        );

        Ok((header, entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_checkpoint_creation() {
        let temp_dir = TempDir::new()
            .context("Failed to create temp directory")
            .unwrap();
        let manager = CheckpointManager::new(temp_dir.path());
        manager
            .init()
            .await
            .context("Failed to initialize checkpoint manager")
            .unwrap();

        let collections = vec![CollectionCheckpointState {
            name: "test_collection".to_string(),
            vector_count: 1000,
            last_lsn: 100,
            dimension: 768,
            engine: "sst".to_string(),
        }];

        let info = manager
            .create_checkpoint("test_checkpoint", 100, collections)
            .await
            .context("Failed to create checkpoint")
            .unwrap();

        assert_eq!(info.name, "test_checkpoint");
        assert_eq!(info.checkpoint_lsn, 100);
        assert_eq!(info.collections.len(), 1);
    }

    #[tokio::test]
    async fn test_checkpoint_list() {
        let temp_dir = TempDir::new()
            .context("Failed to create temp directory")
            .unwrap();
        let manager = CheckpointManager::new(temp_dir.path());
        manager
            .init()
            .await
            .context("Failed to initialize checkpoint manager")
            .unwrap();

        // Create multiple checkpoints
        for i in 0..3 {
            let name = format!("checkpoint_{}", i);
            manager
                .create_checkpoint(&name, i as u64 * 100, vec![])
                .await
                .with_context(|| format!("Failed to create checkpoint '{}'", name))
                .unwrap();
        }

        let list = manager.list_checkpoints().await;
        assert_eq!(list.len(), 3);
    }

    #[tokio::test]
    async fn test_delta_save_load() {
        let temp_dir = TempDir::new()
            .context("Failed to create temp directory")
            .unwrap();
        let manager = CheckpointManager::new(temp_dir.path());
        manager
            .init()
            .await
            .context("Failed to initialize checkpoint manager")
            .unwrap();

        let delta_path = temp_dir
            .path()
            .join("test.delta")
            .to_string_lossy()
            .to_string();

        let entries = vec![
            DeltaEntry {
                lsn: 1,
                collection_id: "col1".to_string(),
                operation: "upsert".to_string(),
                vector_data: Some(vec![1, 2, 3, 4]),
                vector_ids: None,
                collection_config: None,
            },
            DeltaEntry {
                lsn: 2,
                collection_id: "col1".to_string(),
                operation: "delete".to_string(),
                vector_data: None,
                vector_ids: Some(vec!["id1".to_string()]),
                collection_config: None,
            },
        ];

        let info = manager
            .save_delta(&delta_path, entries.clone(), None, 1, 2)
            .await
            .context("Failed to save delta")
            .unwrap();

        assert_eq!(info.entry_count, 2);
        assert_eq!(info.start_lsn, 1);
        assert_eq!(info.end_lsn, 2);

        // Load and verify
        let (header, loaded_entries) = manager
            .load_delta(&delta_path)
            .await
            .context("Failed to load delta")
            .unwrap();
        assert_eq!(header.entry_count, 2);
        assert_eq!(loaded_entries.len(), 2);
        assert_eq!(loaded_entries[0].lsn, 1);
        assert_eq!(loaded_entries[1].lsn, 2);
    }
}
