// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Raft snapshot management

use raft::eraftpb::{ConfState, SnapshotMetadata};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;

use crate::core::VectorDBError;

type Result<T> = std::result::Result<T, VectorDBError>;
type ProximaDBError = VectorDBError;

/// Snapshot manager for Raft consensus
pub struct SnapshotManager {
    /// Directory for storing snapshots
    snapshot_dir: PathBuf,

    /// Maximum number of snapshots to retain
    max_snapshots: usize,

    /// Minimum log entries before triggering snapshot
    snapshot_threshold: u64,

    /// Current snapshot metadata
    current_snapshot: Arc<RwLock<Option<SnapshotInfo>>>,

    /// Snapshot creation in progress
    snapshot_in_progress: Arc<RwLock<bool>>,
}

/// Information about a snapshot
#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    /// Snapshot metadata
    pub metadata: SnapshotMetadata,

    /// File path
    pub file_path: PathBuf,

    /// File size in bytes
    pub file_size: u64,

    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Checksum for integrity verification
    pub checksum: String,
}

/// Snapshot data structure
#[derive(Debug)]
pub struct SnapshotData {
    /// State machine data
    pub state: Vec<u8>,

    /// Configuration state
    pub conf_state: ConfState,

    /// Applied index
    pub applied_index: u64,

    /// Applied term
    pub applied_term: u64,

    /// Metadata
    pub metadata: HashMap<String, String>,
}

impl SnapshotManager {
    /// Create a new snapshot manager
    pub async fn new<P: AsRef<Path>>(
        snapshot_dir: P,
        max_snapshots: usize,
        snapshot_threshold: u64,
    ) -> Result<Self> {
        let snapshot_dir = snapshot_dir.as_ref().to_path_buf();

        // Ensure snapshot directory exists
        fs::create_dir_all(&snapshot_dir)
            .await
            .map_err(|e| ProximaDBError::Storage(crate::core::StorageError::DiskIO(e)))?;

        Ok(Self {
            snapshot_dir,
            max_snapshots,
            snapshot_threshold,
            current_snapshot: Arc::new(RwLock::new(None)),
            snapshot_in_progress: Arc::new(RwLock::new(false)),
        })
    }

    /// Check if a snapshot should be created
    pub async fn should_create_snapshot(&self, log_size: u64) -> bool {
        let in_progress = *self.snapshot_in_progress.read().await;
        !in_progress && log_size >= self.snapshot_threshold
    }

    /// Create a new snapshot
    pub async fn create_snapshot(
        &self,
        state_data: Vec<u8>,
        conf_state: ConfState,
        applied_index: u64,
        applied_term: u64,
    ) -> Result<SnapshotInfo> {
        // Check if snapshot creation is already in progress
        {
            let mut in_progress = self.snapshot_in_progress.write().await;
            if *in_progress {
                return Err(ProximaDBError::Consensus(
                    crate::core::ConsensusError::Raft(
                        "Snapshot creation already in progress".to_string(),
                    ),
                ));
            }
            *in_progress = true;
        }

        let result = self
            .create_snapshot_internal(state_data, conf_state, applied_index, applied_term)
            .await;

        // Clear in-progress flag
        {
            let mut in_progress = self.snapshot_in_progress.write().await;
            *in_progress = false;
        }

        result
    }

    /// Internal snapshot creation logic
    async fn create_snapshot_internal(
        &self,
        state_data: Vec<u8>,
        conf_state: ConfState,
        applied_index: u64,
        applied_term: u64,
    ) -> Result<SnapshotInfo> {
        let now = chrono::Utc::now();
        let filename = format!("snapshot_{}_{}.db", applied_index, applied_term);
        let file_path = self.snapshot_dir.join(&filename);

        // Create snapshot data
        let snapshot_data = SnapshotData {
            state: state_data,
            conf_state: conf_state.clone(),
            applied_index,
            applied_term,
            metadata: HashMap::new(),
        };

        // Serialize snapshot data (simple custom serialization for now)
        let serialized_data = snapshot_data.state;

        // Write to file
        fs::write(&file_path, &serialized_data)
            .await
            .map_err(|e| ProximaDBError::Storage(crate::core::StorageError::DiskIO(e)))?;

        // Calculate checksum
        let checksum = self.calculate_checksum(&serialized_data);

        // Create snapshot metadata
        let mut metadata = SnapshotMetadata::default();
        metadata.index = applied_index;
        metadata.term = applied_term;
        metadata.conf_state = Some(conf_state).into();

        let snapshot_info = SnapshotInfo {
            metadata,
            file_path: file_path.clone(),
            file_size: serialized_data.len() as u64,
            created_at: now,
            checksum,
        };

        // Update current snapshot
        {
            let mut current = self.current_snapshot.write().await;
            *current = Some(snapshot_info.clone());
        }

        tracing::info!(
            "Created snapshot at index {} in file {:?}",
            applied_index,
            file_path
        );

        // Clean up old snapshots
        self.cleanup_old_snapshots().await?;

        Ok(snapshot_info)
    }

    /// Load a snapshot from disk
    pub async fn load_snapshot(&self, snapshot_path: &Path) -> Result<SnapshotData> {
        // Read file
        let data = fs::read(snapshot_path)
            .await
            .map_err(|e| ProximaDBError::Storage(crate::core::StorageError::DiskIO(e)))?;

        // Deserialize (simple custom deserialization for now)
        let snapshot_data = SnapshotData {
            state: data,
            conf_state: ConfState::default(),
            applied_index: 0, // TODO: implement proper metadata storage
            applied_term: 0,
            metadata: HashMap::new(),
        };

        tracing::info!(
            "Loaded snapshot from {:?}, index: {}",
            snapshot_path,
            snapshot_data.applied_index
        );

        Ok(snapshot_data)
    }

    /// Get the current snapshot if available
    pub async fn get_current_snapshot(&self) -> Option<SnapshotInfo> {
        self.current_snapshot.read().await.clone()
    }

    /// Install a snapshot received from leader
    pub async fn install_snapshot(
        &self,
        snapshot_data: Vec<u8>,
        metadata: SnapshotMetadata,
    ) -> Result<()> {
        let filename = format!("installed_snapshot_{}_{}.db", metadata.index, metadata.term);
        let file_path = self.snapshot_dir.join(&filename);

        // Write snapshot data to file
        fs::write(&file_path, &snapshot_data)
            .await
            .map_err(|e| ProximaDBError::Storage(crate::core::StorageError::DiskIO(e)))?;

        // Extract index before move
        let snapshot_index = metadata.index;

        // Create snapshot info
        let snapshot_info = SnapshotInfo {
            metadata,
            file_path,
            file_size: snapshot_data.len() as u64,
            created_at: chrono::Utc::now(),
            checksum: self.calculate_checksum(&snapshot_data),
        };

        // Update current snapshot
        {
            let mut current = self.current_snapshot.write().await;
            *current = Some(snapshot_info);
        }

        tracing::info!("Installed snapshot at index {}", snapshot_index);

        Ok(())
    }

    /// List all available snapshots
    pub async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        let mut snapshots = Vec::new();

        let mut entries = fs::read_dir(&self.snapshot_dir)
            .await
            .map_err(|e| ProximaDBError::Storage(crate::core::StorageError::DiskIO(e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ProximaDBError::Storage(crate::core::StorageError::DiskIO(e)))?
        {
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("db") {
                if let Ok(snapshot_info) = self.parse_snapshot_info(&path).await {
                    snapshots.push(snapshot_info);
                }
            }
        }

        // Sort by index (newest first)
        snapshots.sort_by(|a, b| b.metadata.index.cmp(&a.metadata.index));

        Ok(snapshots)
    }

    /// Delete a specific snapshot
    pub async fn delete_snapshot(&self, snapshot_path: &Path) -> Result<()> {
        fs::remove_file(snapshot_path)
            .await
            .map_err(|e| ProximaDBError::Storage(crate::core::StorageError::DiskIO(e)))?;

        tracing::info!("Deleted snapshot {:?}", snapshot_path);
        Ok(())
    }

    /// Clean up old snapshots, keeping only the most recent ones
    async fn cleanup_old_snapshots(&self) -> Result<()> {
        let snapshots = self.list_snapshots().await?;

        if snapshots.len() <= self.max_snapshots {
            return Ok(());
        }

        // Delete oldest snapshots
        for snapshot in snapshots.iter().skip(self.max_snapshots) {
            self.delete_snapshot(&snapshot.file_path).await?;
        }

        tracing::info!(
            "Cleaned up {} old snapshots",
            snapshots.len().saturating_sub(self.max_snapshots)
        );

        Ok(())
    }

    /// Parse snapshot info from file path
    async fn parse_snapshot_info(&self, path: &Path) -> Result<SnapshotInfo> {
        let metadata = fs::metadata(path)
            .await
            .map_err(|e| ProximaDBError::Storage(crate::core::StorageError::DiskIO(e)))?;

        // Try to parse index and term from filename
        let filename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

        let (index, term) = self.parse_filename(filename);

        let mut raft_metadata = SnapshotMetadata::default();
        raft_metadata.index = index;
        raft_metadata.term = term;

        Ok(SnapshotInfo {
            metadata: raft_metadata,
            file_path: path.to_path_buf(),
            file_size: metadata.len(),
            created_at: chrono::DateTime::from(metadata.created().unwrap_or(std::time::UNIX_EPOCH)),
            checksum: "".to_string(), // Would need to calculate from file
        })
    }

    /// Parse index and term from filename
    fn parse_filename(&self, filename: &str) -> (u64, u64) {
        // Parse "snapshot_{index}_{term}" format
        if let Some(captures) = regex::Regex::new(r"snapshot_(\d+)_(\d+)")
            .unwrap()
            .captures(filename)
        {
            let index = captures
                .get(1)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
            let term = captures
                .get(2)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
            (index, term)
        } else {
            (0, 0)
        }
    }

    /// Calculate checksum for data integrity
    fn calculate_checksum(&self, data: &[u8]) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        data.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

/// Snapshot statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotStats {
    pub total_snapshots: usize,
    pub total_size_bytes: u64,
    pub latest_snapshot_index: u64,
    pub oldest_snapshot_index: u64,
    pub avg_snapshot_size_bytes: u64,
    pub last_snapshot_created: Option<chrono::DateTime<chrono::Utc>>,
}
