//! Enhanced crash recovery for HELIX engine
//!
//! This module provides checkpoint-based crash recovery with redo log support
//! for consistent state recovery after failures.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::storage::persistence::filesystem::FileSystem;
use super::SStableMetadata;

/// Checkpoint metadata for crash recovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// Checkpoint version
    pub version: u64,
    /// Timestamp of checkpoint creation
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last compaction timestamp
    pub last_compaction: Option<chrono::DateTime<chrono::Utc>>,
    /// Level metadata snapshot
    pub level_metadata: HashMap<usize, Vec<SStableMetadata>>,
    /// Active PCA model version
    pub pca_model_version: Option<u32>,
    /// Redo log position
    pub redo_log_position: u64,
}

/// Redo log entry for operation replay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RedoLogEntry {
    /// Flush operation completed
    FlushCompleted {
        timestamp: chrono::DateTime<chrono::Utc>,
        level: usize,
        file_path: PathBuf,
        num_vectors: usize,
    },
    /// Compaction operation completed
    CompactionCompleted {
        timestamp: chrono::DateTime<chrono::Utc>,
        from_level: usize,
        to_level: usize,
        input_files: Vec<PathBuf>,
        output_files: Vec<PathBuf>,
    },
    /// File deletion
    FileDeleted {
        timestamp: chrono::DateTime<chrono::Utc>,
        file_path: PathBuf,
    },
    /// PCA model update
    PCAModelUpdated {
        timestamp: chrono::DateTime<chrono::Utc>,
        version: u32,
    },
}

/// Recovery state machine states
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryState {
    /// Initial state, checking for recovery need
    Initial,
    /// Loading checkpoint
    LoadingCheckpoint,
    /// Replaying redo log
    ReplayingRedoLog,
    /// Verifying consistency
    VerifyingConsistency,
    /// Recovery complete
    Complete,
    /// Recovery failed
    Failed(String),
}

/// Crash recovery manager for HELIX engine
pub struct CrashRecoveryManager {
    /// Filesystem interface
    filesystem: Arc<dyn FileSystem>,
    /// Data directory
    data_dir: PathBuf,
    /// Recovery directory for checkpoints and logs
    recovery_dir: PathBuf,
    /// Current checkpoint
    current_checkpoint: Arc<RwLock<Option<CheckpointMetadata>>>,
    /// Redo log entries
    redo_log: Arc<RwLock<Vec<RedoLogEntry>>>,
    /// Recovery state
    recovery_state: Arc<RwLock<RecoveryState>>,
    /// Checkpoint interval in seconds
    checkpoint_interval_secs: u64,
    /// Last checkpoint time
    last_checkpoint_time: Arc<RwLock<chrono::DateTime<chrono::Utc>>>,
}

impl CrashRecoveryManager {
    /// Create a new crash recovery manager
    pub fn new(
        filesystem: Arc<dyn FileSystem>,
        data_dir: PathBuf,
        checkpoint_interval_secs: u64,
    ) -> Self {
        let recovery_dir = data_dir.join("recovery");
        
        Self {
            filesystem,
            data_dir,
            recovery_dir,
            current_checkpoint: Arc::new(RwLock::new(None)),
            redo_log: Arc::new(RwLock::new(Vec::new())),
            recovery_state: Arc::new(RwLock::new(RecoveryState::Initial)),
            checkpoint_interval_secs,
            last_checkpoint_time: Arc::new(RwLock::new(chrono::Utc::now())),
        }
    }

    /// Initialize recovery manager and check for recovery need
    pub async fn initialize(&self) -> Result<bool> {
        // Create recovery directory if it doesn't exist
        self.filesystem.create_dir_all(&self.recovery_dir.to_string_lossy()).await?;
        
        // Check for existing checkpoint
        let checkpoint_path = self.recovery_dir.join("checkpoint.json");
        let needs_recovery = self.filesystem.exists(&checkpoint_path.to_string_lossy()).await?;
        
        if needs_recovery {
            info!("Found existing checkpoint, recovery may be needed");
            *self.recovery_state.write().await = RecoveryState::LoadingCheckpoint;
        }
        
        Ok(needs_recovery)
    }

    /// Perform crash recovery
    pub async fn recover(&self) -> Result<HashMap<usize, Vec<SStableMetadata>>> {
        info!("Starting crash recovery");
        
        // Load checkpoint
        let checkpoint = self.load_checkpoint().await?;
        *self.current_checkpoint.write().await = Some(checkpoint.clone());
        *self.recovery_state.write().await = RecoveryState::ReplayingRedoLog;
        
        // Load and replay redo log
        let redo_entries = self.load_redo_log().await?;
        let recovered_state = self.replay_redo_log(checkpoint, redo_entries).await?;
        
        // Verify consistency
        *self.recovery_state.write().await = RecoveryState::VerifyingConsistency;
        self.verify_consistency(&recovered_state).await?;
        
        // Clean up recovery files
        self.cleanup_recovery_files().await?;
        
        *self.recovery_state.write().await = RecoveryState::Complete;
        info!("Crash recovery completed successfully");
        
        Ok(recovered_state)
    }

    /// Create a checkpoint
    pub async fn create_checkpoint(
        &self,
        level_metadata: &HashMap<usize, Vec<SStableMetadata>>,
        pca_model_version: Option<u32>,
    ) -> Result<()> {
        // Check if checkpoint is needed
        let now = chrono::Utc::now();
        let last_checkpoint = *self.last_checkpoint_time.read().await;
        
        if (now - last_checkpoint).num_seconds() < self.checkpoint_interval_secs as i64 {
            return Ok(());
        }
        
        info!("Creating checkpoint");
        
        let checkpoint = CheckpointMetadata {
            version: now.timestamp() as u64,
            created_at: now,
            last_compaction: None,
            level_metadata: level_metadata.clone(),
            pca_model_version,
            redo_log_position: self.redo_log.read().await.len() as u64,
        };
        
        // Save checkpoint to disk
        let checkpoint_path = self.recovery_dir.join("checkpoint.json");
        let checkpoint_json = serde_json::to_vec_pretty(&checkpoint)?;
        self.filesystem.write(&checkpoint_path.to_string_lossy(), &checkpoint_json, None).await?;
        
        *self.current_checkpoint.write().await = Some(checkpoint);
        *self.last_checkpoint_time.write().await = now;
        
        // Truncate redo log
        self.truncate_redo_log().await?;
        
        info!("Checkpoint created successfully");
        Ok(())
    }

    /// Append entry to redo log
    pub async fn append_redo_log(&self, entry: RedoLogEntry) -> Result<()> {
        // Append to in-memory log
        self.redo_log.write().await.push(entry.clone());
        
        // Persist to disk
        let redo_log_path = self.recovery_dir.join("redo.log");
        let entry_json = serde_json::to_vec(&entry)?;
        
        // Append with newline separator
        let mut data = entry_json;
        data.push(b'\n');
        
        // Use append mode if filesystem supports it
        if self.filesystem.exists(&redo_log_path.to_string_lossy()).await? {
            let existing = self.filesystem.read(&redo_log_path.to_string_lossy()).await?;
            let mut combined = existing;
            combined.extend_from_slice(&data);
            self.filesystem.write(&redo_log_path.to_string_lossy(), &combined, None).await?;
        } else {
            self.filesystem.write(&redo_log_path.to_string_lossy(), &data, None).await?;
        }
        
        Ok(())
    }

    /// Get recovery state
    pub async fn get_state(&self) -> RecoveryState {
        self.recovery_state.read().await.clone()
    }

    // Private helper methods

    async fn load_checkpoint(&self) -> Result<CheckpointMetadata> {
        let checkpoint_path = self.recovery_dir.join("checkpoint.json");
        let checkpoint_data = self.filesystem.read(&checkpoint_path.to_string_lossy()).await
            .context("Failed to read checkpoint file")?;
        
        let checkpoint: CheckpointMetadata = serde_json::from_slice(&checkpoint_data)
            .context("Failed to parse checkpoint")?;
        
        info!("Loaded checkpoint version {}", checkpoint.version);
        Ok(checkpoint)
    }

    async fn load_redo_log(&self) -> Result<Vec<RedoLogEntry>> {
        let redo_log_path = self.recovery_dir.join("redo.log");
        
        if !self.filesystem.exists(&redo_log_path.to_string_lossy()).await? {
            return Ok(Vec::new());
        }
        
        let redo_data = self.filesystem.read(&redo_log_path.to_string_lossy()).await?;
        let mut entries = Vec::new();
        
        // Parse line-delimited JSON entries
        for line in redo_data.split(|&b| b == b'\n') {
            if !line.is_empty() {
                match serde_json::from_slice::<RedoLogEntry>(line) {
                    Ok(entry) => entries.push(entry),
                    Err(e) => warn!("Failed to parse redo log entry: {}", e),
                }
            }
        }
        
        info!("Loaded {} redo log entries", entries.len());
        Ok(entries)
    }

    async fn replay_redo_log(
        &self,
        mut checkpoint: CheckpointMetadata,
        entries: Vec<RedoLogEntry>,
    ) -> Result<HashMap<usize, Vec<SStableMetadata>>> {
        info!("Replaying {} redo log entries", entries.len());
        
        let mut level_metadata = checkpoint.level_metadata;
        
        for entry in entries {
            match entry {
                RedoLogEntry::FlushCompleted { level, file_path, num_vectors, .. } => {
                    // Add new flush file to level metadata
                    let metadata = SStableMetadata {
                        path: file_path,
                        level,
                        hilbert_range: None,
                        num_vectors,
                        size_bytes: 0, // Will be updated on next checkpoint
                        created_at: chrono::Utc::now(),
                        blocks: Vec::new(),
                        bloom_filter: None,
                    };
                    
                    level_metadata.entry(level)
                        .or_insert_with(Vec::new)
                        .push(metadata);
                }
                
                RedoLogEntry::CompactionCompleted { from_level, to_level, input_files, output_files, .. } => {
                    // Remove input files from source levels
                    if let Some(from_files) = level_metadata.get_mut(&from_level) {
                        from_files.retain(|f| !input_files.contains(&f.path));
                    }
                    if let Some(to_files) = level_metadata.get_mut(&to_level) {
                        to_files.retain(|f| !input_files.contains(&f.path));
                    }
                    
                    // Add output files to target level
                    for output_path in output_files {
                        let metadata = SStableMetadata {
                            path: output_path,
                            level: to_level,
                            hilbert_range: None,
                            num_vectors: 0, // Will be updated on next checkpoint
                            size_bytes: 0,
                            created_at: chrono::Utc::now(),
                            blocks: Vec::new(),
                            bloom_filter: None,
                        };
                        
                        level_metadata.entry(to_level)
                            .or_insert_with(Vec::new)
                            .push(metadata);
                    }
                }
                
                RedoLogEntry::FileDeleted { file_path, .. } => {
                    // Remove deleted file from all levels
                    for (_level, files) in level_metadata.iter_mut() {
                        files.retain(|f| f.path != file_path);
                    }
                }
                
                RedoLogEntry::PCAModelUpdated { version, .. } => {
                    // Update PCA model version in checkpoint
                    checkpoint.pca_model_version = Some(version);
                }
            }
        }
        
        Ok(level_metadata)
    }

    async fn verify_consistency(&self, level_metadata: &HashMap<usize, Vec<SStableMetadata>>) -> Result<()> {
        info!("Verifying consistency of recovered state");
        
        let mut missing_files = Vec::new();
        let mut total_files = 0;
        
        // Check that all files in metadata actually exist
        for (level, files) in level_metadata {
            for file_meta in files {
                total_files += 1;
                
                if !self.filesystem.exists(&file_meta.path.to_string_lossy()).await? {
                    error!("Missing file at level {}: {:?}", level, file_meta.path);
                    missing_files.push(file_meta.path.clone());
                }
            }
        }
        
        if !missing_files.is_empty() {
            return Err(anyhow::anyhow!(
                "Consistency check failed: {} missing files out of {}",
                missing_files.len(),
                total_files
            ));
        }
        
        info!("Consistency check passed: all {} files present", total_files);
        Ok(())
    }

    async fn cleanup_recovery_files(&self) -> Result<()> {
        // Remove checkpoint and redo log after successful recovery
        let checkpoint_path = self.recovery_dir.join("checkpoint.json");
        let redo_log_path = self.recovery_dir.join("redo.log");
        
        if self.filesystem.exists(&checkpoint_path.to_string_lossy()).await? {
            self.filesystem.delete(&checkpoint_path.to_string_lossy()).await?;
        }
        
        if self.filesystem.exists(&redo_log_path.to_string_lossy()).await? {
            self.filesystem.delete(&redo_log_path.to_string_lossy()).await?;
        }
        
        info!("Cleaned up recovery files");
        Ok(())
    }

    async fn truncate_redo_log(&self) -> Result<()> {
        // Clear in-memory log
        self.redo_log.write().await.clear();
        
        // Delete redo log file
        let redo_log_path = self.recovery_dir.join("redo.log");
        if self.filesystem.exists(&redo_log_path.to_string_lossy()).await? {
            self.filesystem.delete(&redo_log_path.to_string_lossy()).await?;
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::FilesystemFactory;

    #[tokio::test]
    async fn test_crash_recovery_manager() {
        let filesystem = FilesystemFactory::create_local().unwrap();
        let data_dir = PathBuf::from("/tmp/helix_recovery_test");
        
        let manager = CrashRecoveryManager::new(filesystem, data_dir, 300);
        
        // Initialize should create recovery directory
        let needs_recovery = manager.initialize().await.unwrap();
        assert!(!needs_recovery); // No existing checkpoint
        
        // Verify initial state
        assert_eq!(manager.get_state().await, RecoveryState::Initial);
    }

    #[tokio::test]
    async fn test_redo_log_append() {
        let filesystem = FilesystemFactory::create_local().unwrap();
        let data_dir = PathBuf::from("/tmp/helix_redo_test");
        
        let manager = CrashRecoveryManager::new(filesystem, data_dir, 300);
        manager.initialize().await.unwrap();
        
        // Append a flush entry
        let entry = RedoLogEntry::FlushCompleted {
            timestamp: chrono::Utc::now(),
            level: 0,
            file_path: PathBuf::from("L0_test.helix"),
            num_vectors: 100,
        };
        
        manager.append_redo_log(entry).await.unwrap();
        
        // Verify entry was added
        assert_eq!(manager.redo_log.read().await.len(), 1);
    }
}