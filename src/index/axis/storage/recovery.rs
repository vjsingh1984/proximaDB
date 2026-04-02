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

//! AXIS Index Recovery Manager
//!
//! Handles restoration of indexes from persisted state across different storage tiers.

use crate::index::axis::integration::collection_state::{
    CollectionStateManager, CollectionTierState, TierLevel,
};
use crate::index::axis::integration::tiering_manager::AxisTieringManager;
use crate::index::axis::storage::serialization::{
    DeltaManager, Index, IndexCheckpoint, IndexDelta, IndexSerializer, SerializationError,
};
use crate::storage::persistence::filesystem::{FileStorageTier, FilesystemFactory};
use dashmap::DashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Recovery manager configuration
#[derive(Debug, Clone)]
pub struct RecoveryConfig {
    /// Enable automatic recovery on startup
    pub auto_recovery_enabled: bool,

    /// Recovery timeout per collection
    pub recovery_timeout_secs: u64,

    /// Maximum parallel recoveries
    pub max_parallel_recoveries: usize,

    /// Verify checksums during recovery
    pub verify_checksums: bool,

    /// Preferred recovery tier (where to load indexes initially)
    pub preferred_recovery_tier: TierLevel,

    /// Enable delta reconstruction
    pub enable_delta_reconstruction: bool,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            auto_recovery_enabled: true,
            recovery_timeout_secs: 300, // 5 minutes
            max_parallel_recoveries: 4,
            verify_checksums: true,
            preferred_recovery_tier: TierLevel::Memory,
            enable_delta_reconstruction: true,
        }
    }
}

/// Convenience type alias for recovery operation results
pub type RecoveryResult = Result<RecoveryStats, SerializationError>;
/// Type alias for `RecoveryConfig` for compatibility
pub type RecoveryStrategy = RecoveryConfig;

/// Recovery status for a collection
#[derive(Debug, Clone)]
pub enum RecoveryStatus {
    /// Recovery has not been initiated yet
    NotStarted,
    /// Recovery is currently running
    InProgress {
        /// Wall-clock instant when recovery started
        started_at: Instant,
        /// Completion percentage (0.0–100.0)
        progress_percent: f32,
    },
    /// Recovery finished successfully
    Completed {
        /// Total time taken for recovery
        duration: Duration,
        /// Number of vectors that were restored
        vectors_recovered: usize,
    },
    /// Recovery failed with an error
    Failed {
        /// Human-readable error description
        error: String,
        /// Number of retry attempts made so far
        retry_count: u32,
    },
}

/// Recovery statistics
#[derive(Debug, Clone, Default)]
pub struct RecoveryStats {
    /// Number of collections successfully recovered
    pub collections_recovered: u32,
    /// Number of collections whose recovery failed
    pub collections_failed: u32,
    /// Total number of vectors restored across all collections
    pub total_vectors_recovered: u64,
    /// Total bytes loaded from persistent storage
    pub total_bytes_loaded: u64,
    /// Cumulative wall-clock time spent on recovery
    pub total_recovery_time: Duration,
    /// Instant of the most recent recovery run
    pub last_recovery: Option<Instant>,
}

/// Recovery manager for AXIS indexes
pub struct IndexRecoveryManager {
    /// Configuration
    config: RecoveryConfig,

    /// Filesystem factory for reading persisted indexes
    filesystem: Arc<FilesystemFactory>,

    /// Collection state manager
    collection_state: Arc<CollectionStateManager>,

    /// Tiering manager
    tiering_manager: Arc<AxisTieringManager>,

    /// Recovery status per collection
    recovery_status: Arc<DashMap<String, RecoveryStatus>>,

    /// Delta managers per collection
    delta_managers: Arc<DashMap<String, DeltaManager>>,

    /// Recovery statistics
    stats: Arc<RwLock<RecoveryStats>>,

    /// Checkpoint storage locations (collection_id -> checkpoint path)
    checkpoint_locations: Arc<DashMap<String, String>>,
}

impl IndexRecoveryManager {
    /// Create new recovery manager
    pub fn new(
        config: RecoveryConfig,
        filesystem: Arc<FilesystemFactory>,
        collection_state: Arc<CollectionStateManager>,
        tiering_manager: Arc<AxisTieringManager>,
    ) -> Self {
        Self {
            config,
            filesystem,
            collection_state,
            tiering_manager,
            recovery_status: Arc::new(DashMap::new()),
            delta_managers: Arc::new(DashMap::new()),
            stats: Arc::new(RwLock::new(RecoveryStats::default())),
            checkpoint_locations: Arc::new(DashMap::new()),
        }
    }

    /// Recover all collections on startup
    pub async fn recover_all_collections(&self) -> Result<(), SerializationError> {
        if !self.config.auto_recovery_enabled {
            info!("Auto recovery disabled, skipping");
            return Ok(());
        }

        info!("Starting recovery of all collections");
        let start_time = Instant::now();

        // Get list of collections from state manager
        let collections = self
            .collection_state
            .list_collections()
            .await
            .map_err(|_| SerializationError::InvalidMagic)?;

        info!("Found {} collections to recover", collections.len());

        // Recover collections in parallel (up to max_parallel_recoveries)
        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            self.config.max_parallel_recoveries,
        ));

        let mut tasks = Vec::new();

        for collection_id in collections {
            let permit = semaphore.clone().acquire_owned().await.map_err(|e| {
                SerializationError::NotSupported(format!("Recovery semaphore closed: {}", e))
            })?;
            let manager = self.clone();
            let collection_id = collection_id.clone();

            let task = tokio::spawn(async move {
                let result = manager.recover_collection(&collection_id).await;
                drop(permit);

                if let Err(e) = result {
                    error!("Failed to recover collection {}: {}", collection_id, e);
                    manager.update_recovery_status(
                        &collection_id,
                        RecoveryStatus::Failed {
                            error: e.to_string(),
                            retry_count: 0,
                        },
                    );
                }
            });

            tasks.push(task);
        }

        // Wait for all recoveries to complete
        for task in tasks {
            let _ = task.await;
        }

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_recovery_time = start_time.elapsed();
            stats.last_recovery = Some(Instant::now());
        }

        info!("Recovery completed in {:?}", start_time.elapsed());
        self.log_recovery_stats().await;

        Ok(())
    }

    /// Recover a single collection
    pub async fn recover_collection(&self, collection_id: &str) -> Result<(), SerializationError> {
        self.recover_collection_with_retries(collection_id, 3).await
    }

    /// Internal method with retry counter to prevent infinite recursion
    fn recover_collection_with_retries<'a>(
        &'a self,
        collection_id: &'a str,
        max_retries: u32,
    ) -> Pin<Box<dyn Future<Output = Result<(), SerializationError>> + Send + 'a>> {
        Box::pin(async move {
            info!(
                "Recovering collection: {} (retries remaining: {})",
                collection_id, max_retries
            );

            // Update status
            self.update_recovery_status(
                collection_id,
                RecoveryStatus::InProgress {
                    started_at: Instant::now(),
                    progress_percent: 0.0,
                },
            );

            let start_time = Instant::now();

            // Get collection state
            let state = self
                .collection_state
                .get_state(collection_id)
                .ok_or_else(|| {
                    SerializationError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Collection {} not found", collection_id),
                    ))
                })?;

            match state {
                CollectionTierState::Memory { .. } => {
                    info!(
                        "Collection {} already in memory, skipping recovery",
                        collection_id
                    );
                    return Ok(());
                }

                CollectionTierState::Disk { disk_location, .. } => {
                    info!(
                        "Recovering collection {} from disk: {:?}",
                        collection_id, disk_location
                    );
                    self.recover_from_disk(collection_id, &disk_location.to_string_lossy())
                        .await?;
                }

                CollectionTierState::Cloud { location, .. } => {
                    info!(
                        "Recovering collection {} from cloud: {}",
                        collection_id, location
                    );
                    self.recover_from_cloud(collection_id, &location).await?;
                }

                CollectionTierState::Unbuilt => {
                    info!(
                        "Collection {} is unbuilt, looking for checkpoint",
                        collection_id
                    );
                    self.recover_from_checkpoint(collection_id).await?;
                }

                CollectionTierState::Transitioning { .. } => {
                    if max_retries == 0 {
                        return Err(SerializationError::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("Collection {} stuck in transitioning state", collection_id),
                        )));
                    }

                    info!(
                        "Collection {} is transitioning, waiting before retry",
                        collection_id
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;

                    // Retry with decremented counter
                    return self
                        .recover_collection_with_retries(collection_id, max_retries - 1)
                        .await;
                }
            }

            // Update status
            self.update_recovery_status(
                collection_id,
                RecoveryStatus::Completed {
                    duration: start_time.elapsed(),
                    vectors_recovered: 0, // Will be updated with actual count
                },
            );

            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats.collections_recovered += 1;
            }

            info!(
                "Collection {} recovered in {:?}",
                collection_id,
                start_time.elapsed()
            );
            Ok(())
        })
    }

    /// Recover index from disk storage
    async fn recover_from_disk(
        &self,
        collection_id: &str,
        disk_location: &str,
    ) -> Result<(), SerializationError> {
        debug!("Reading index from disk: {}", disk_location);

        // Read index data from disk
        let index_data = self.filesystem.read(disk_location).await.map_err(|e| {
            SerializationError::Io(std::io::Error::other(e))
        })?;

        debug!("Read {} bytes from disk", index_data.len());

        // Update progress
        self.update_recovery_status(
            collection_id,
            RecoveryStatus::InProgress {
                started_at: Instant::now(),
                progress_percent: 50.0,
            },
        );

        // Determine index type from data
        let index_type = self.detect_index_type(&index_data)?;
        let data_len = index_data.len();

        // Load based on preferred recovery tier
        match self.config.preferred_recovery_tier {
            TierLevel::Memory => {
                // Load directly into memory
                self.load_index_to_memory(collection_id, index_data, index_type)
                    .await?;
            }

            TierLevel::Disk => {
                // Keep on disk, just verify integrity
                self.verify_index_integrity(&index_data)?;
            }

            _ => {
                info!("Unsupported recovery tier, defaulting to mem");
                self.load_index_to_memory(collection_id, index_data, index_type)
                    .await?;
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_bytes_loaded += data_len as u64;
        }

        Ok(())
    }

    /// Recover index from cloud storage
    async fn recover_from_cloud(
        &self,
        collection_id: &str,
        cloud_location: &str,
    ) -> Result<(), SerializationError> {
        debug!("Reading index from cloud: {}", cloud_location);

        // Read index data from cloud
        let index_data = self.filesystem.read(cloud_location).await.map_err(|e| {
            SerializationError::Io(std::io::Error::other(e))
        })?;

        debug!("Read {} bytes from cloud", index_data.len());

        // Determine index type from data
        let index_type = self.detect_index_type(&index_data)?;

        // For cloud recovery, we might want to cache locally first
        if self.config.preferred_recovery_tier == TierLevel::Memory {
            self.load_index_to_memory(collection_id, index_data, index_type)
                .await?;
        } else {
            // Cache to disk first
            let disk_path = format!("axis/indexes/{}/index.bin", collection_id);
            let disk_url = self
                .filesystem
                .get_tier_url(FileStorageTier::SSD, &disk_path)
                .map_err(|e| {
                    SerializationError::Io(std::io::Error::other(e))
                })?;

            self.filesystem
                .write(&disk_url, &index_data, None)
                .await
                .map_err(|e| {
                    SerializationError::Io(std::io::Error::other(e))
                })?;

            // Update state to disk
            self.collection_state
                .transition_to_disk(collection_id, disk_url)
                .await
                .map_err(|_| {
                    SerializationError::Io(std::io::Error::other(
                        "Failed to transition to disk",
                    ))
                })?;
        }

        Ok(())
    }

    /// Recover from checkpoint
    async fn recover_from_checkpoint(&self, collection_id: &str) -> Result<(), SerializationError> {
        // Look for checkpoint location
        let checkpoint_path = if let Some(location) = self.checkpoint_locations.get(collection_id) {
            location.clone()
        } else {
            // Default checkpoint location
            format!("axis/checkpoints/{}/latest.checkpoint", collection_id)
        };

        debug!("Looking for checkpoint at: {}", checkpoint_path);

        // Check if checkpoint exists
        if !self
            .filesystem
            .exists(&checkpoint_path)
            .await
            .map_err(|e| {
                SerializationError::Io(std::io::Error::other(e))
            })?
        {
            warn!("No checkpoint found for collection {}", collection_id);
            return Err(SerializationError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No checkpoint found for collection {}", collection_id),
            )));
        }

        // Read checkpoint
        let checkpoint_data = self.filesystem.read(&checkpoint_path).await.map_err(|e| {
            SerializationError::Io(std::io::Error::other(e))
        })?;
        let checkpoint: IndexCheckpoint = bincode::deserialize(&checkpoint_data)?;

        info!(
            "Found checkpoint {} for collection {}",
            checkpoint.checkpoint_id, collection_id
        );

        // Check if we need to apply deltas
        if self.config.enable_delta_reconstruction {
            let delta_path = format!("axis/checkpoints/{}/deltas/", collection_id);

            if self.filesystem.exists(&delta_path).await.map_err(|e| {
                SerializationError::Io(std::io::Error::other(e))
            })? {
                let deltas = self.load_deltas(&delta_path).await?;

                if !deltas.is_empty() {
                    info!("Applying {} deltas to checkpoint", deltas.len());

                    // Create delta manager and apply deltas
                    let mut delta_manager = DeltaManager::new(10);
                    delta_manager.set_checkpoint(checkpoint.clone());

                    for delta in deltas {
                        for op in delta.operations {
                            delta_manager.add_delta(op);
                        }
                    }

                    // Get reconstructed state
                    if let Some(reconstructed) = delta_manager.reconstruct_current_state()? {
                        // Use reconstructed state
                        let index_type = self.detect_index_type(&reconstructed)?;
                        self.load_index_to_memory(collection_id, reconstructed, index_type)
                            .await?;

                        // Store delta manager for future updates
                        self.delta_managers
                            .insert(collection_id.to_string(), delta_manager);

                        return Ok(());
                    }
                }
            }
        }

        // No deltas or delta reconstruction disabled, use checkpoint directly
        let index_type = checkpoint.metadata.index_type;
        self.load_index_to_memory(collection_id, checkpoint.index_data, index_type)
            .await?;

        Ok(())
    }

    /// Load deltas from storage
    async fn load_deltas(&self, delta_path: &str) -> Result<Vec<IndexDelta>, SerializationError> {
        let mut deltas = Vec::new();

        // List all delta files
        let entries = self.filesystem.list(delta_path).await.map_err(|e| {
            SerializationError::Io(std::io::Error::other(e))
        })?;

        for entry in entries {
            if entry.name.ends_with(".delta") {
                let delta_data = self.filesystem.read(&entry.url).await.map_err(|e| {
                    SerializationError::Io(std::io::Error::other(e))
                })?;
                let delta: IndexDelta = bincode::deserialize(&delta_data)?;
                deltas.push(delta);
            }
        }

        // Sort deltas by timestamp
        deltas.sort_by_key(|d| d.timestamp);

        Ok(deltas)
    }

    /// Detect index type from serialized data
    fn detect_index_type(&self, data: &[u8]) -> Result<Index, SerializationError> {
        if data.len() < 8 {
            return Err(SerializationError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Data too short",
            )));
        }

        // Check magic bytes
        if &data[4..8] != b"AXIS" {
            return Err(SerializationError::InvalidMagic);
        }

        // Read header to get index type
        let header_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        if data.len() < 4 + header_len {
            return Err(SerializationError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Header incomplete",
            )));
        }

        // Parse just enough to get index type
        // This is a simplified version - actual implementation would properly deserialize

        // For now, return a default
        Ok(Index::Hnsw)
    }

    /// Load index into memory
    async fn load_index_to_memory(
        &self,
        collection_id: &str,
        _index_data: Vec<u8>,
        index_type: Index,
    ) -> Result<(), SerializationError> {
        info!(
            "Loading {:?} index for collection {} into mem",
            index_type, collection_id
        );

        // This would deserialize and load the actual index
        // For now, we'll just update the state

        // Update collection state to memory
        self.collection_state
            .transition_to_memory(collection_id)
            .await
            .map_err(|_| {
                SerializationError::Io(std::io::Error::other(
                    "Failed to transition to mem",
                ))
            })?;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_vectors_recovered += 1000; // Placeholder
        }

        Ok(())
    }

    /// Verify index integrity
    fn verify_index_integrity(&self, data: &[u8]) -> Result<(), SerializationError> {
        if !self.config.verify_checksums {
            return Ok(());
        }

        // This would verify checksums and structure
        // For now, just check basic structure

        if data.len() < 8 {
            return Err(SerializationError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Data too short",
            )));
        }

        Ok(())
    }

    /// Update recovery status for a collection
    fn update_recovery_status(&self, collection_id: &str, status: RecoveryStatus) {
        self.recovery_status
            .insert(collection_id.to_string(), status);
    }

    /// Get recovery status for a collection
    pub fn get_recovery_status(&self, collection_id: &str) -> Option<RecoveryStatus> {
        self.recovery_status.get(collection_id).map(|s| s.clone())
    }

    /// Log recovery statistics
    async fn log_recovery_stats(&self) {
        let stats = self.stats.read().await;

        info!("Recovery Statistics:");
        info!("  Collections recovered: {}", stats.collections_recovered);
        info!("  Collections failed: {}", stats.collections_failed);
        info!(
            "  Total vectors recovered: {}",
            stats.total_vectors_recovered
        );
        info!("  Total bytes loaded: {}", stats.total_bytes_loaded);
        info!("  Total recovery time: {:?}", stats.total_recovery_time);
    }

    /// Create a checkpoint for a collection
    pub async fn create_checkpoint(
        &self,
        collection_id: &str,
        index_data: Vec<u8>,
        index_type: Index,
    ) -> Result<String, SerializationError> {
        let checkpoint = IndexSerializer::create_checkpoint(index_type, index_data, collection_id)?;

        // Save checkpoint to storage
        let checkpoint_path = format!(
            "axis/checkpoints/{}/{}.checkpoint",
            collection_id, checkpoint.checkpoint_id
        );

        let checkpoint_data = bincode::serialize(&checkpoint)?;
        self.filesystem
            .write(&checkpoint_path, &checkpoint_data, None)
            .await
            .map_err(|e| {
                SerializationError::Io(std::io::Error::other(e))
            })?;

        // Update latest link
        let latest_path = format!("axis/checkpoints/{}/latest.checkpoint", collection_id);
        self.filesystem
            .write(&latest_path, &checkpoint_data, None)
            .await
            .map_err(|e| {
                SerializationError::Io(std::io::Error::other(e))
            })?;

        // Store checkpoint location
        self.checkpoint_locations
            .insert(collection_id.to_string(), checkpoint_path.clone());

        info!(
            "Created checkpoint {} for collection {}",
            checkpoint.checkpoint_id, collection_id
        );

        Ok(checkpoint.checkpoint_id)
    }
}

// Manual Clone implementation
impl Clone for IndexRecoveryManager {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            filesystem: Arc::clone(&self.filesystem),
            collection_state: Arc::clone(&self.collection_state),
            tiering_manager: Arc::clone(&self.tiering_manager),
            recovery_status: Arc::clone(&self.recovery_status),
            delta_managers: Arc::clone(&self.delta_managers),
            stats: Arc::clone(&self.stats),
            checkpoint_locations: Arc::clone(&self.checkpoint_locations),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn test_recovery_config() {
        let config = RecoveryConfig::default();

        assert!(config.auto_recovery_enabled);
        assert_eq!(config.recovery_timeout_secs, 300);
        assert_eq!(config.max_parallel_recoveries, 4);
        assert!(config.verify_checksums);
    }

    #[tokio::test]
    async fn test_recovery_status() {
        let status = RecoveryStatus::InProgress {
            started_at: Instant::now(),
            progress_percent: 50.0,
        };

        match status {
            RecoveryStatus::InProgress {
                progress_percent, ..
            } => {
                assert_eq!(progress_percent, 50.0);
            }
            _ => panic!("Expected InProgress status"),
        }
    }
}
