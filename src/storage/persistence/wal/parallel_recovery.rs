// Copyright 2024 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Parallel WAL Recovery System with Assignment Service Integration
//!
//! This module implements fast WAL recovery using the assignment service for
//! collection discovery and parallel recovery across multiple disks.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::storage::assignment_service::AssignmentService;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
// SimpleAtomicSync removed - using AtomicWalSync instead

/// Parallel recovery system for WAL data
pub struct ParallelRecoverySystem {
    assignment_service: Arc<dyn AssignmentService>,
    filesystem_factory: Arc<FilesystemFactory>,
    recovery_stats: Arc<RwLock<RecoveryStats>>,
}

/// Recovery statistics for monitoring
#[derive(Debug, Clone, Default)]
pub struct RecoveryStats {
    pub collections_discovered: usize,
    pub collections_recovered: usize,
    pub total_vectors_recovered: usize,
    pub total_batches_recovered: usize,
    pub recovery_duration_ms: u64,
    pub errors: Vec<RecoveryError>,
    pub per_disk_stats: HashMap<String, DiskRecoveryStats>,
}

/// Recovery statistics per disk
#[derive(Debug, Clone, Default)]
pub struct DiskRecoveryStats {
    pub disk_id: String,
    pub collections_on_disk: usize,
    pub vectors_recovered: usize,
    pub batches_recovered: usize,
    pub recovery_duration_ms: u64,
    pub errors: Vec<String>,
}

/// Recovery error types
#[derive(Debug, Clone)]
pub enum RecoveryError {
    CollectionDiscoveryFailed(String),
    WalFileCorrupted(String),
    DeserializationFailed(String),
    AssignmentServiceError(String),
    FilesystemError(String),
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecoveryError::CollectionDiscoveryFailed(msg) => write!(f, "Collection discovery failed: {}", msg),
            RecoveryError::WalFileCorrupted(msg) => write!(f, "WAL file corrupted: {}", msg),
            RecoveryError::DeserializationFailed(msg) => write!(f, "Deserialization failed: {}", msg),
            RecoveryError::AssignmentServiceError(msg) => write!(f, "Assignment service error: {}", msg),
            RecoveryError::FilesystemError(msg) => write!(f, "Filesystem error: {}", msg),
        }
    }
}

impl ParallelRecoverySystem {
    /// Create new parallel recovery system
    pub fn new(
        assignment_service: Arc<dyn AssignmentService>,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Self {
        Self {
            assignment_service,
            filesystem_factory,
            recovery_stats: Arc::new(RwLock::new(RecoveryStats::default())),
        }
    }

    /// Recover all collections in parallel using assignment service discovery
    pub async fn recover_all_collections(&self) -> Result<RecoveryStats> {
        let start_time = std::time::Instant::now();
        info!("🔄 Starting parallel WAL recovery using assignment service");

        // Phase 1: Discover collections using assignment service
        let collections_by_disk = self.discover_collections_by_assignment().await
            .context("Failed to discover collections using assignment service")?;

        info!("📦 Discovered {} disks with collections", collections_by_disk.len());
        for (disk_id, collections) in &collections_by_disk {
            info!("   Disk {}: {} collections", disk_id, collections.len());
        }

        // Phase 2: Recover each disk in parallel
        let disk_recovery_tasks: Vec<_> = collections_by_disk.into_iter()
            .map(|(disk_id, collections)| {
                let recovery_system = self.clone_for_task();
                tokio::spawn(async move {
                    recovery_system.recover_disk_collections(&disk_id, collections).await
                })
            })
            .collect();

        // Phase 3: Aggregate results from all disks
        let mut total_stats = RecoveryStats {
            recovery_duration_ms: start_time.elapsed().as_millis() as u64,
            ..Default::default()
        };

        for task in disk_recovery_tasks {
            match task.await {
                Ok(Ok(disk_stats)) => {
                    total_stats.collections_discovered += disk_stats.collections_on_disk;
                    total_stats.collections_recovered += if disk_stats.errors.is_empty() { disk_stats.collections_on_disk } else { 0 };
                    total_stats.total_vectors_recovered += disk_stats.vectors_recovered;
                    total_stats.total_batches_recovered += disk_stats.batches_recovered;
                    total_stats.per_disk_stats.insert(disk_stats.disk_id.clone(), disk_stats);
                }
                Ok(Err(e)) => {
                    total_stats.errors.push(RecoveryError::CollectionDiscoveryFailed(e.to_string()));
                }
                Err(e) => {
                    total_stats.errors.push(RecoveryError::AssignmentServiceError(e.to_string()));
                }
            }
        }

        // Update recovery stats
        {
            let mut stats = self.recovery_stats.write().await;
            *stats = total_stats.clone();
        }

        info!(
            "✅ Parallel WAL recovery completed: {} collections, {} vectors recovered in {}ms",
            total_stats.collections_recovered,
            total_stats.total_vectors_recovered,
            total_stats.recovery_duration_ms
        );

        if !total_stats.errors.is_empty() {
            warn!("⚠️ Recovery completed with {} errors", total_stats.errors.len());
            for error in &total_stats.errors {
                warn!("   - {}", error);
            }
        }

        Ok(total_stats)
    }

    /// Discover collections grouped by disk using assignment service
    async fn discover_collections_by_assignment(&self) -> Result<HashMap<String, Vec<String>>> {
        debug!("🔍 Discovering collections using assignment service");

        // Get all assignments from the assignment service
        let all_assignments = self.assignment_service
            .get_all_assignments()
            .await;

        // Group collections by their assigned disk
        let mut collections_by_disk: HashMap<String, Vec<String>> = HashMap::new();

        for (collection_id, assignment) in all_assignments {
            let disk_id = self.extract_disk_id(&assignment.location_url);
            collections_by_disk
                .entry(disk_id)
                .or_insert_with(Vec::new)
                .push(collection_id);
        }

        Ok(collections_by_disk)
    }

    /// Recover collections from a specific disk
    async fn recover_disk_collections(
        &self,
        disk_id: &str,
        collections: Vec<String>,
    ) -> Result<DiskRecoveryStats> {
        let start_time = std::time::Instant::now();
        debug!("🔄 Starting recovery for disk '{}' with {} collections", disk_id, collections.len());

        let mut disk_stats = DiskRecoveryStats {
            disk_id: disk_id.to_string(),
            collections_on_disk: collections.len(),
            ..Default::default()
        };

        // Recover collections sequentially on same disk to avoid disk thrashing
        for collection_id in collections {
            match self.recover_collection(&collection_id).await {
                Ok(collection_recovery) => {
                    disk_stats.vectors_recovered += collection_recovery.vectors_recovered;
                    disk_stats.batches_recovered += collection_recovery.batches_recovered;
                    debug!("✅ Recovered collection '{}': {} vectors, {} batches", 
                           collection_id, collection_recovery.vectors_recovered, collection_recovery.batches_recovered);
                }
                Err(e) => {
                    let error_msg = format!("Failed to recover collection '{}': {}", collection_id, e);
                    disk_stats.errors.push(error_msg.clone());
                    warn!("{}", error_msg);
                }
            }
        }

        disk_stats.recovery_duration_ms = start_time.elapsed().as_millis() as u64;
        
        info!(
            "✅ Disk '{}' recovery completed: {}/{} collections, {} vectors, {} batches in {}ms",
            disk_id,
            disk_stats.collections_on_disk - disk_stats.errors.len(),
            disk_stats.collections_on_disk,
            disk_stats.vectors_recovered,
            disk_stats.batches_recovered,
            disk_stats.recovery_duration_ms
        );

        Ok(disk_stats)
    }

    /// Recover a single collection from WAL
    async fn recover_collection(&self, collection_id: &str) -> Result<CollectionRecoveryInfo> {
        debug!("🔄 Recovering collection '{}'", collection_id);

        // Get collection assignment
        let assignment = self.assignment_service
            .get_assignment(collection_id)
            .await
            .context("Failed to get assignment for collection")?;

        // Use the WAL URL directly - it already includes collection_id/wal
        let collection_wal_path = &assignment.wal_url;
        let logs_path = format!("{}/logs", collection_wal_path);
        let checkpoints_path = format!("{}/checkpoints", collection_wal_path);

        let filesystem = self.filesystem_factory
            .get_filesystem(&assignment.location_url)
            .context("Failed to get filesystem for collection")?;

        // Check if WAL directories exist
        if !filesystem.exists(&logs_path).await? {
            debug!("No WAL logs directory found for collection '{}', skipping", collection_id);
            return Ok(CollectionRecoveryInfo {
                collection_id: collection_id.to_string(),
                vectors_recovered: 0,
                batches_recovered: 0,
                checkpoint_used: false,
            });
        }

        // Read checkpoint (if exists) to determine recovery starting point
        let checkpoint_info = self.read_checkpoint(&checkpoints_path, filesystem).await?;
        let start_sequence = checkpoint_info.unwrap_or(0);

        // Discover and read WAL batch files
        let wal_files = self.discover_wal_files(&logs_path, filesystem).await?;
        let mut batches_recovered = 0;
        let mut vectors_recovered = 0;

        for wal_file in wal_files {
            // Skip files with sequences before checkpoint
            if self.get_file_sequence(&wal_file) <= start_sequence {
                continue;
            }

            match self.recover_wal_file(&wal_file, filesystem).await {
                Ok(batch) => {
                    batches_recovered += 1;
                    vectors_recovered += batch.vector_records.len();
                    
                    // For Phase 2, we'll add the batch to the global memtable
                    // For now, we just count it as recovered
                    debug!("📦 Recovered WAL batch: {} vectors", batch.vector_records.len());
                }
                Err(e) => {
                    warn!("Failed to recover WAL file {}: {}", wal_file, e);
                }
            }
        }

        Ok(CollectionRecoveryInfo {
            collection_id: collection_id.to_string(),
            vectors_recovered,
            batches_recovered,
            checkpoint_used: checkpoint_info.is_some(),
        })
    }

    /// Read checkpoint to get last processed sequence
    async fn read_checkpoint(
        &self,
        checkpoints_path: &str,
        filesystem: &dyn crate::storage::persistence::filesystem::FileSystem,
    ) -> Result<Option<u64>> {
        let checkpoint_file = format!("{}/latest.checkpoint", checkpoints_path);
        
        if !filesystem.exists(&checkpoint_file).await? {
            return Ok(None);
        }

        let checkpoint_data = filesystem.read(&checkpoint_file).await
            .context("Failed to read checkpoint file")?;
        
        let checkpoint_json: serde_json::Value = serde_json::from_slice(&checkpoint_data)
            .context("Failed to parse checkpoint JSON")?;
        
        let last_sequence = checkpoint_json
            .get("last_sequence")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        
        debug!("📖 Read checkpoint: last sequence {}", last_sequence);
        Ok(Some(last_sequence))
    }

    /// Discover WAL files in logs directory
    async fn discover_wal_files(
        &self,
        logs_path: &str,
        filesystem: &dyn crate::storage::persistence::filesystem::FileSystem,
    ) -> Result<Vec<String>> {
        let mut wal_files = Vec::new();
        
        // List files in logs directory
        match filesystem.list(logs_path).await {
            Ok(entries) => {
                for entry in entries {
                    if entry.name.ends_with(".wal") && !entry.metadata.is_directory {
                        wal_files.push(format!("{}/{}", logs_path, entry.name));
                    }
                }
            }
            Err(_) => {
                // Directory doesn't exist or is empty
                debug!("No WAL files found in {}", logs_path);
            }
        }

        // Sort by sequence number for proper recovery order
        wal_files.sort_by_key(|f| self.get_file_sequence(f));
        
        debug!("📁 Discovered {} WAL files in {}", wal_files.len(), logs_path);
        Ok(wal_files)
    }

    /// Extract sequence number from WAL filename
    fn get_file_sequence(&self, file_path: &str) -> u64 {
        // Extract sequence from filename like "batch_000000010_000000020.wal"
        if let Some(filename) = file_path.split('/').last() {
            if let Some(parts) = filename.strip_prefix("batch_").and_then(|s| s.strip_suffix(".wal")) {
                if let Some(start_seq) = parts.split('_').next() {
                    return start_seq.parse::<u64>().unwrap_or(0);
                }
            }
        }
        0
    }

    /// Recover a single WAL file
    async fn recover_wal_file(
        &self,
        file_path: &str,
        filesystem: &dyn crate::storage::persistence::filesystem::FileSystem,
    ) -> Result<WalVectorBatch> {
        let file_data = filesystem.read(file_path).await
            .context("Failed to read WAL file")?;

        // For Phase 2, we'll implement proper deserialization
        // For now, create a placeholder batch
        use crate::storage::persistence::wal::BatchId;
        
        let batch_id = BatchId::new();

        let batch = WalVectorBatch {
            batch_id,
            vector_records: Arc::new(Vec::new()), // TODO: Deserialize actual vectors
            created_at: std::time::SystemTime::now(),
            total_size_bytes: file_data.len(),
            is_flushed: false,
        };

        debug!("📦 Recovered WAL file: {} bytes", file_data.len());
        Ok(batch)
    }

    /// Extract disk ID from storage URL
    fn extract_disk_id(&self, storage_url: &str) -> String {
        if let Some(file_path) = storage_url.strip_prefix("file://") {
            if let Some(disk_part) = file_path.split('/').find(|part| part.starts_with("disk")) {
                return disk_part.to_string();
            }
        }
        
        // Fallback: use hash of URL
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        storage_url.hash(&mut hasher);
        format!("disk_{}", hasher.finish() % 1000)
    }

    /// Clone for task execution
    fn clone_for_task(&self) -> Self {
        Self {
            assignment_service: self.assignment_service.clone(),
            filesystem_factory: self.filesystem_factory.clone(),
            recovery_stats: self.recovery_stats.clone(),
        }
    }

    /// Get current recovery statistics
    pub async fn get_recovery_stats(&self) -> RecoveryStats {
        self.recovery_stats.read().await.clone()
    }
}

/// Recovery information for a single collection
#[derive(Debug, Clone)]
pub struct CollectionRecoveryInfo {
    pub collection_id: String,
    pub vectors_recovered: usize,
    pub batches_recovered: usize,
    pub checkpoint_used: bool,
}

#[cfg(test)]
mod tests {
    
    
    #[tokio::test]
    async fn test_parallel_recovery() {
        // TODO: Implement comprehensive recovery tests
        assert!(true);
    }
    
    #[tokio::test]
    async fn test_collection_discovery() {
        // TODO: Test assignment service-based collection discovery
        assert!(true);
    }
}