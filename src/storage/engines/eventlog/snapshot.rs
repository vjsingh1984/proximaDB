/*
 * Copyright 2025 Vijaykumar Singh
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

//! # Snapshot Manager for Event Sourcing
//!
//! This module provides snapshot management for efficient state reconstruction,
//! avoiding full event replay for entities with many events.

use proximadb_kernel::error::ProximaDBError;
use crate::storage::engines::eventlog::{EntityId, EventSequence};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Snapshot metadata
#[derive(Debug, Clone)]
pub struct SnapshotMetadata {
    /// Entity this snapshot is for
    pub entity_id: EntityId,

    /// Event sequence at snapshot time
    pub sequence: EventSequence,

    /// Snapshot creation timestamp
    pub created_at: DateTime<Utc>,

    /// Snapshot file path
    pub file_path: PathBuf,

    /// Snapshot size in bytes
    pub size_bytes: usize,
}

/// Snapshot manager for efficient state reconstruction
pub struct SnapshotManager {
    /// Base directory for snapshot storage
    base_dir: PathBuf,

    /// Snapshot metadata index
    snapshots: Arc<RwLock<HashMap<EntityId, Vec<SnapshotMetadata>>>>,

    /// Snapshot statistics
    stats: Arc<RwLock<SnapshotStats>>,
}

/// Snapshot statistics
#[derive(Debug, Clone, Default)]
pub struct SnapshotStats {
    /// Total snapshots created
    pub total_snapshots: u64,

    /// Total size of all snapshots (bytes)
    pub total_size_bytes: u64,

    /// Snapshot hits (avoided event replay)
    pub hits: u64,

    /// Snapshot misses (needed to replay from events)
    pub misses: u64,
}

impl SnapshotManager {
    /// Create a new snapshot manager
    pub fn new(base_dir: PathBuf) -> Result<Self> {
        debug!("Creating snapshot manager at {:?}", base_dir);

        // Create snapshot directory
        std::fs::create_dir_all(&base_dir).map_err(|e| {
            ProximaDBError::Internal(format!("Failed to create snapshot dir: {}", e))
        })?;

        Ok(Self {
            base_dir,
            snapshots: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(SnapshotStats::default())),
        })
    }

    /// Create a snapshot for an entity at a specific sequence
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to snapshot
    /// * `sequence` - Event sequence to snapshot at
    ///
    /// # Returns
    ///
    /// Snapshot metadata
    pub async fn create_snapshot(
        &self,
        entity_id: &EntityId,
        sequence: EventSequence,
    ) -> Result<SnapshotMetadata> {
        info!(
            "Creating snapshot for {} at sequence {}",
            entity_id, sequence
        );

        // In production, we'd:
        // 1. Load all events for the entity
        // 2. Replay them to compute current state
        // 3. Serialize state to snapshot file
        // 4. Update index

        // For now, create a placeholder snapshot
        let snapshot_path = self.get_snapshot_path(entity_id, sequence);

        // Placeholder snapshot data
        let snapshot_data = serde_json::json!({
            "entity_id": entity_id,
            "sequence": sequence,
            "state": "placeholder_state",
            "created_at": Utc::now().to_rfc3339(),
        });

        let serialized = serde_json::to_vec(&snapshot_data).map_err(|e| {
            ProximaDBError::Internal(format!("Snapshot serialization failed: {}", e))
        })?;

        let size_bytes = serialized.len();

        // Create entity directory if it doesn't exist
        if let Some(entity_dir) = snapshot_path.parent() {
            tokio::fs::create_dir_all(entity_dir).await.map_err(|e| {
                ProximaDBError::Internal(format!("Failed to create entity dir: {}", e))
            })?;
        }

        // Write snapshot file
        tokio::fs::write(&snapshot_path, serialized)
            .await
            .map_err(|e| ProximaDBError::Internal(format!("Failed to write snapshot: {}", e)))?;

        let metadata = SnapshotMetadata {
            entity_id: entity_id.clone(),
            sequence,
            created_at: Utc::now(),
            file_path: snapshot_path.clone(),
            size_bytes,
        };

        // Update index
        {
            let mut snapshots = self.snapshots.write().await;
            snapshots
                .entry(entity_id.clone())
                .or_insert_with(Vec::new)
                .push(metadata.clone());
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_snapshots += 1;
            stats.total_size_bytes += size_bytes as u64;
        }

        debug!(
            "Created snapshot for {} at sequence {} ({} bytes)",
            entity_id, sequence, size_bytes
        );

        Ok(metadata)
    }

    /// Load a snapshot for an entity
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to load snapshot for
    /// * `sequence` - Snapshot sequence to load
    ///
    /// # Returns
    ///
    /// Entity state as JSON
    pub async fn load_snapshot(
        &self,
        entity_id: &EntityId,
        sequence: EventSequence,
    ) -> Result<serde_json::Value> {
        debug!(
            "Loading snapshot for {} at sequence {}",
            entity_id, sequence
        );

        // Find snapshot metadata
        let snapshot_path = {
            let snapshots = self.snapshots.read().await;
            snapshots
                .get(entity_id)
                .and_then(|snapshots| snapshots.iter().find(|s| s.sequence == sequence))
                .map(|s| s.file_path.clone())
                .ok_or_else(|| {
                    ProximaDBError::Internal(format!(
                        "Snapshot not found for {} at sequence {}",
                        entity_id, sequence
                    ))
                })?
        };

        // Read snapshot file
        let data = tokio::fs::read(&snapshot_path)
            .await
            .map_err(|e| ProximaDBError::Internal(format!("Failed to read snapshot: {}", e)))?;

        // Deserialize
        let snapshot: serde_json::Value = serde_json::from_slice(&data).map_err(|e| {
            ProximaDBError::Internal(format!("Snapshot deserialization failed: {}", e))
        })?;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.hits += 1;
        }

        debug!("Loaded snapshot for {} at sequence {}", entity_id, sequence);
        Ok(snapshot)
    }

    /// Find the most recent snapshot before a given timestamp
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to find snapshot for
    /// * `before` - Timestamp constraint
    ///
    /// # Returns
    ///
    /// Snapshot sequence if found, None otherwise
    pub async fn find_snapshot_before(
        &self,
        entity_id: &EntityId,
        before: DateTime<Utc>,
    ) -> Result<Option<EventSequence>> {
        let snapshots = self.snapshots.read().await;

        if let Some(entity_snapshots) = snapshots.get(entity_id) {
            // Find most recent snapshot before the given time
            let snapshot = entity_snapshots
                .iter()
                .filter(|s| s.created_at <= before)
                .max_by_key(|s| s.sequence);

            Ok(snapshot.map(|s| s.sequence))
        } else {
            Ok(None)
        }
    }

    /// Get all snapshots for an entity
    pub async fn get_entity_snapshots(
        &self,
        entity_id: &EntityId,
    ) -> Result<Vec<SnapshotMetadata>> {
        let snapshots = self.snapshots.read().await;

        // Return empty Vec if no snapshots exist for this entity (valid state)
        Ok(match snapshots.get(entity_id) {
            Some(entity_snapshots) => entity_snapshots.clone(),
            None => Vec::new(),
        })
    }

    /// Delete old snapshots to save space
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to clean up
    /// * `keep_latest` - Number of recent snapshots to keep
    ///
    /// # Returns
    ///
    /// Number of snapshots deleted
    pub async fn cleanup_old_snapshots(
        &self,
        entity_id: &EntityId,
        keep_latest: usize,
    ) -> Result<usize> {
        debug!(
            "Cleaning up snapshots for {} (keeping latest {})",
            entity_id, keep_latest
        );

        let mut snapshots = self.snapshots.write().await;

        if let Some(entity_snapshots) = snapshots.get_mut(entity_id) {
            let original_count = entity_snapshots.len();

            if entity_snapshots.len() > keep_latest {
                // Sort by sequence descending and keep latest N
                entity_snapshots.sort_by(|a, b| b.sequence.cmp(&a.sequence));
                entity_snapshots.truncate(keep_latest);

                // Delete files for removed snapshots
                let to_delete = &entity_snapshots[keep_latest..];
                for snapshot in to_delete {
                    let _ = tokio::fs::remove_file(&snapshot.file_path).await;
                }

                let deleted = original_count - keep_latest;
                debug!("Deleted {} old snapshots for {}", deleted, entity_id);
                return Ok(deleted);
            }
        }

        Ok(0)
    }

    /// Get snapshot statistics
    pub async fn get_stats(&self) -> SnapshotStats {
        self.stats.read().await.clone()
    }

    /// Get storage path for a snapshot
    fn get_snapshot_path(&self, entity_id: &EntityId, sequence: EventSequence) -> PathBuf {
        // Sanitize entity_id for filesystem
        let safe_id = entity_id.replace(':', "_");
        self.base_dir
            .join(format!("entity_{}", safe_id))
            .join(format!("snapshot_{:010}.json", sequence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_manager_creation() {
        let base_dir = PathBuf::from("/tmp/test_snapshot_manager");
        let manager = SnapshotManager::new(base_dir.clone())
            .expect("Failed to create snapshot manager for test");
        assert_eq!(manager.base_dir, base_dir);
    }

    #[tokio::test]
    async fn test_create_snapshot() {
        let base_dir = PathBuf::from("/tmp/test_create_snapshot");
        let manager = SnapshotManager::new(base_dir.clone())
            .expect("Failed to create snapshot manager for test");

        let metadata = manager
            .create_snapshot(&"entity:test".to_string(), 100)
            .await
            .expect("Failed to create snapshot for test");

        assert_eq!(metadata.entity_id, "entity:test");
        assert_eq!(metadata.sequence, 100);
        assert!(metadata.file_path.exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(base_dir);
    }

    #[tokio::test]
    async fn test_load_snapshot() {
        let base_dir = PathBuf::from("/tmp/test_load_snapshot");
        let manager = SnapshotManager::new(base_dir.clone())
            .expect("Failed to create snapshot manager for test");

        // Create snapshot
        manager
            .create_snapshot(&"entity:test".to_string(), 100)
            .await
            .expect("Failed to create snapshot for test");

        // Load snapshot
        let state = manager
            .load_snapshot(&"entity:test".to_string(), 100)
            .await
            .expect("Failed to load snapshot for test");
        assert_eq!(state["entity_id"], "entity:test");
        assert_eq!(state["sequence"], 100);

        // Cleanup
        let _ = std::fs::remove_dir_all(base_dir);
    }

    #[tokio::test]
    async fn test_cleanup_old_snapshots() {
        let base_dir = PathBuf::from("/tmp/test_cleanup_snapshots");
        let manager = SnapshotManager::new(base_dir.clone())
            .expect("Failed to create snapshot manager for test");

        // Create multiple snapshots
        for i in 1..=5 {
            manager
                .create_snapshot(&"entity:test".to_string(), i * 100)
                .await
                .expect("Failed to create snapshot for test");
        }

        // Cleanup, keep only latest 2
        let deleted = manager
            .cleanup_old_snapshots(&"entity:test".to_string(), 2)
            .await
            .expect("Failed to cleanup old snapshots for test");
        assert_eq!(deleted, 3);

        // Verify only 2 remain
        let snapshots = manager
            .get_entity_snapshots(&"entity:test".to_string())
            .await
            .expect("Failed to get entity snapshots for test");
        assert_eq!(snapshots.len(), 2);

        // Cleanup
        let _ = std::fs::remove_dir_all(base_dir);
    }
}
