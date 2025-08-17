// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Checkpoint Module for Filestore Backend
//!
//! This module handles the checkpoint creation process for the filestore metadata backend.
//! It's responsible for:
//! - Merging incremental operations into snapshots
//! - Archiving old snapshots and incremental logs
//! - Cleaning up old files
//! - Maintaining the last N snapshots for recovery
//!
//! The checkpoint creation process is atomic and blocks all API operations during execution.

use anyhow::Result;
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::proto::proximadb::Collection as Collection;
use crate::storage::metadata::backends::filestore_backend::{
    IncrementalOperation, OperationType,
};
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};

// NOTE: Using unified CompactionConfig from unified_types.rs
// This specific metadata checkpoint config extends the base config
#[derive(Debug, Clone)]
pub struct MetadataCheckpointConfig {
    /// Base checkpoint configuration (reusing CompactionConfig for compatibility)
    pub base: crate::core::CompactionConfig,
    /// Maximum number of incremental operations before triggering checkpoint
    pub max_incremental_operations: usize,
    /// Maximum size of incremental logs before triggering checkpoint (bytes)
    pub max_incremental_size_bytes: usize,
    /// Number of snapshots to keep in archive
    pub keep_snapshots: usize,
    /// Enable compression for snapshots
    pub compress_snapshots: bool,
}

impl Default for MetadataCheckpointConfig {
    fn default() -> Self {
        Self {
            base: crate::core::CompactionConfig::default(),
            max_incremental_operations: 1000,
            max_incremental_size_bytes: 100 * 1024 * 1024, // 100MB
            keep_snapshots: 5,
            compress_snapshots: true,
        }
    }
}

/// Checkpoint statistics
#[derive(Debug, Default, Clone)]
pub struct CheckpointStats {
    pub last_checkpoint_time: Option<chrono::DateTime<chrono::Utc>>,
    pub total_checkpoints: u64,
    pub operations_compacted: u64,
    pub snapshots_created: u64,
    pub archives_created: u64,
    pub bytes_compacted: u64,
    pub current_incremental_count: usize,
    pub current_incremental_size: usize,
}

/// Checkpoint manager for filestore backend
pub struct FilestoreCheckpointManager {
    config: MetadataCheckpointConfig,
    filesystem: Arc<FilesystemFactory>,
    filestore_url: String,
    metadata_path: PathBuf,
    stats: CheckpointStats,
}

impl FilestoreCheckpointManager {
    /// Create new checkpoint manager
    pub fn new(
        config: MetadataCheckpointConfig,
        filesystem: Arc<FilesystemFactory>,
        filestore_url: String,
    ) -> Self {
        Self {
            config,
            filesystem,
            filestore_url,
            metadata_path: PathBuf::from("metadata_info"),
            stats: CheckpointStats::default(),
        }
    }

    /// Check if checkpoint is needed
    pub async fn needs_checkpoint(&self) -> Result<bool> {
        if !self.config.base.enable_background_compaction {
            return Ok(false);
        }

        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;
        let incremental_dir = self.metadata_path.join("incremental");

        let mut count = 0;
        let mut total_size = 0;

        if let Ok(entries) = fs.list(&incremental_dir.to_string_lossy()).await {
            for entry in entries {
                if entry.name.starts_with("op_") && entry.name.ends_with(".oplog") {
                    count += 1;
                    total_size += entry.metadata.size as usize;
                }
            }
        }

        Ok(count >= self.config.max_incremental_operations
            || total_size >= self.config.max_incremental_size_bytes)
    }

    /// Create checkpoint - merge incremental operations into new snapshot
    pub async fn create_checkpoint(&mut self) -> Result<CheckpointResult> {
        info!("📸 Starting filestore checkpoint creation");
        let start_time = std::time::Instant::now();

        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;

        // Step 1: Load current snapshot
        let mut memtable = self.load_current_snapshot(fs).await?;
        let initial_count = memtable.len();

        // Step 2: Apply incremental operations
        let (ops_count, ops_size) = self.apply_incremental_operations(fs, &mut memtable).await?;

        // Step 3: Create new snapshot
        self.create_new_snapshot(fs, &memtable).await?;

        // Step 4: Archive old files
        let archive_path = self.archive_current_state(fs).await?;

        // Step 5: Clean up old archives
        self.cleanup_old_archives(fs).await?;

        // Update stats
        self.stats.last_checkpoint_time = Some(Utc::now());
        self.stats.total_checkpoints += 1;
        self.stats.operations_compacted += ops_count as u64;
        self.stats.snapshots_created += 1;
        self.stats.archives_created += 1;
        self.stats.bytes_compacted += ops_size as u64;
        self.stats.current_incremental_count = 0;
        self.stats.current_incremental_size = 0;

        let result = CheckpointResult {
            duration: start_time.elapsed(),
            initial_collections: initial_count,
            final_collections: memtable.len(),
            operations_compacted: ops_count,
            bytes_compacted: ops_size,
            archive_path: Some(archive_path),
        };

        info!(
            "✅ Checkpoint completed: {} operations in {:?}",
            ops_count,
            start_time.elapsed()
        );

        Ok(result)
    }

    /// Load current snapshot
    async fn load_current_snapshot(
        &self,
        fs: &dyn FileSystem,
    ) -> Result<BTreeMap<String, Collection>> {
        let snapshot_path = self
            .metadata_path
            .join("snapshots/current_collections.meta");
        let mut memtable = BTreeMap::new();

        if fs.exists(&snapshot_path.to_string_lossy()).await? {
            debug!("Loading current snapshot");
            let data = fs.read(&snapshot_path.to_string_lossy()).await?;

            let reader = apache_avro::Reader::new(&data[..])?;
            for value in reader {
                let record: Collection = apache_avro::from_value(&value?)?;
                memtable.insert(record.id.clone(), record);
            }

            debug!("Loaded {} collections from snapshot", memtable.len());
        }

        Ok(memtable)
    }

    /// Apply incremental operations to memtable
    async fn apply_incremental_operations(
        &self,
        fs: &dyn FileSystem,
        memtable: &mut BTreeMap<String, Collection>,
    ) -> Result<(usize, usize)> {
        let incremental_dir = self.metadata_path.join("incremental");
        let mut operations = Vec::new();
        let mut total_size = 0;

        // Read all incremental operation files
        if let Ok(entries) = fs.list(&incremental_dir.to_string_lossy()).await {
            let mut op_files: Vec<_> = entries
                .into_iter()
                .filter(|e| e.name.starts_with("op_") && e.name.ends_with(".oplog"))
                .collect();

            // Sort by filename (which includes sequence number)
            op_files.sort_by(|a, b| a.name.cmp(&b.name));

            for entry in op_files {
                let path = incremental_dir.join(&entry.name);
                total_size += entry.metadata.size as usize;

                if let Ok(data) = fs.read(&path.to_string_lossy()).await {
                    let reader = apache_avro::Reader::new(&data[..])?;

                    for value in reader {
                        if let Ok(avro_value) = value {
                            // Parse the Avro record manually
                            if let apache_avro::types::Value::Record(fields) = avro_value {
                                let operation = self.parse_incremental_operation(fields)?;
                                operations.push(operation);
                            }
                        }
                    }
                }
            }
        }

        // Apply operations in order
        for op in &operations {
            match op.operation_type {
                OperationType::Create | OperationType::Update => {
                    if let Some(ref record) = op.collection_data {
                        memtable.insert(record.id.clone(), record.clone());
                    }
                }
                OperationType::Delete => {
                    memtable.remove(&op.collection_id);
                }
            }
        }

        Ok((operations.len(), total_size))
    }

    /// Parse incremental operation from Avro record fields
    fn parse_incremental_operation(
        &self,
        fields: Vec<(String, apache_avro::types::Value)>,
    ) -> Result<IncrementalOperation> {
        let mut op_type_str = String::new();
        let mut sequence_number = 0i64;
        let mut timestamp = String::new();
        let mut collection_id = String::new();
        let mut collection_data_json: Option<String> = None;

        for (field_name, field_value) in fields {
            match field_name.as_str() {
                "operation_type" => {
                    if let apache_avro::types::Value::String(s) = field_value {
                        op_type_str = s;
                    }
                }
                "sequence_number" => {
                    if let apache_avro::types::Value::Long(n) = field_value {
                        sequence_number = n;
                    }
                }
                "timestamp" => {
                    if let apache_avro::types::Value::String(s) = field_value {
                        timestamp = s;
                    }
                }
                "collection_id" => {
                    if let apache_avro::types::Value::String(s) = field_value {
                        collection_id = s;
                    }
                }
                "collection_data" => match field_value {
                    apache_avro::types::Value::String(s) => {
                        collection_data_json = Some(s);
                    }
                    apache_avro::types::Value::Null => {
                        collection_data_json = None;
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        let op_type = match op_type_str.as_str() {
            "Create" => OperationType::Create,
            "Update" => OperationType::Update,
            "Delete" => OperationType::Delete,
            _ => return Err(anyhow::anyhow!("Invalid operation type: {}", op_type_str)),
        };

        let collection_data = collection_data_json
            .and_then(|json| serde_json::from_str::<Collection>(&json).ok());

        // Parse timestamp string to i64
        let timestamp_i64 = timestamp.parse::<i64>()
            .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());

        Ok(IncrementalOperation {
            operation_type: op_type,
            sequence: sequence_number as u64,
            timestamp: timestamp_i64,
            collection_id: collection_id,
            collection_data: collection_data,
        })
    }

    /// Create new snapshot from memtable
    async fn create_new_snapshot(
        &self,
        fs: &dyn FileSystem,
        memtable: &BTreeMap<String, Collection>,
    ) -> Result<()> {
        let snapshot_path = self
            .metadata_path
            .join("snapshots/current_collections.meta");
        let temp_path = self
            .metadata_path
            .join("snapshots/current_collections.meta.tmp");

        info!("Creating new snapshot with {} collections", memtable.len());

        // Serialize all collections to protobuf
        let mut collections = Vec::new();
        for record in memtable.values() {
            collections.push(record.clone());
        }
        
        // Create a wrapper message for all collections
        let snapshot = crate::proto::proximadb::CollectionSnapshot {
            collections,
            version: 1,
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        
        // Serialize to protobuf binary
        let data = if self.config.compress_snapshots {
            // Compress with zstd
            let proto_data = prost::Message::encode_to_vec(&snapshot);
            zstd::encode_all(proto_data.as_slice(), 3)?
        } else {
            prost::Message::encode_to_vec(&snapshot)
        };

        // Write atomically
        fs.write(&temp_path.to_string_lossy(), &data, None).await?;
        fs.move_file(
            &temp_path.to_string_lossy(),
            &snapshot_path.to_string_lossy(),
        )
        .await?;

        info!("✅ Created new snapshot: {} bytes", data.len());
        Ok(())
    }

    /// Archive current state with timestamp
    async fn archive_current_state(&self, fs: &dyn FileSystem) -> Result<String> {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let archive_dir = self.metadata_path.join(format!("archive/{}", timestamp));

        // Create archive directory
        fs.create_dir(&archive_dir.to_string_lossy()).await?;

        info!("📦 Archiving to: {}", archive_dir.display());

        // Copy current snapshot if exists
        let current_snapshot = self
            .metadata_path
            .join("snapshots/current_collections.meta");
        if fs.exists(&current_snapshot.to_string_lossy()).await? {
            let archive_snapshot = archive_dir.join("snapshot_collections.meta");
            fs.copy(
                &current_snapshot.to_string_lossy(),
                &archive_snapshot.to_string_lossy(),
            )
            .await?;
        }

        // Move incremental files
        let incremental_dir = self.metadata_path.join("incremental");
        if let Ok(entries) = fs.list(&incremental_dir.to_string_lossy()).await {
            let archive_incremental = archive_dir.join("incremental");
            fs.create_dir(&archive_incremental.to_string_lossy())
                .await?;

            for entry in entries {
                if entry.name.ends_with(".oplog") {
                    let src = incremental_dir.join(&entry.name);
                    let dst = archive_incremental.join(&entry.name);

                    // Copy then delete
                    fs.copy(&src.to_string_lossy(), &dst.to_string_lossy())
                        .await?;
                    fs.delete(&src.to_string_lossy()).await?;
                }
            }
        }

        Ok(archive_dir.to_string_lossy().to_string())
    }

    /// Clean up old archives keeping only the most recent N
    async fn cleanup_old_archives(&self, fs: &dyn FileSystem) -> Result<()> {
        let archive_base_dir = self.metadata_path.join("archive");

        if let Ok(entries) = fs.list(&archive_base_dir.to_string_lossy()).await {
            let mut archive_dirs: Vec<_> = entries
                .into_iter()
                .filter(|e| e.metadata.is_directory)
                .map(|e| e.name)
                .collect();

            // Sort by timestamp (directory names)
            archive_dirs.sort();

            // Remove oldest directories if we have too many
            while archive_dirs.len() > self.config.keep_snapshots {
                if let Some(oldest) = archive_dirs.first() {
                    let path = archive_base_dir.join(oldest);

                    match fs.delete(&path.to_string_lossy()).await {
                        Ok(_) => {
                            debug!("🗑️ Removed old archive: {}", oldest);
                            archive_dirs.remove(0);
                        }
                        Err(e) => {
                            warn!("Failed to remove archive {}: {}", oldest, e);
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get checkpoint statistics
    pub fn stats(&self) -> &CheckpointStats {
        &self.stats
    }

    /// Update current incremental stats (called by filestore backend)
    pub async fn update_incremental_stats(&mut self) -> Result<()> {
        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;
        let incremental_dir = self.metadata_path.join("incremental");

        let mut count = 0;
        let mut size = 0;

        if let Ok(entries) = fs.list(&incremental_dir.to_string_lossy()).await {
            for entry in entries {
                if entry.name.starts_with("op_") && entry.name.ends_with(".oplog") {
                    count += 1;
                    size += entry.metadata.size as usize;
                }
            }
        }

        self.stats.current_incremental_count = count;
        self.stats.current_incremental_size = size;

        Ok(())
    }
}

/// Result of a checkpoint operation
#[derive(Debug)]
pub struct CheckpointResult {
    pub duration: std::time::Duration,
    pub initial_collections: usize,
    pub final_collections: usize,
    pub operations_compacted: usize,
    pub bytes_compacted: usize,
    pub archive_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_config_defaults() {
        let config = MetadataCheckpointConfig::default();
        assert!(config.base.enable_background_compaction);
        assert_eq!(config.max_incremental_operations, 1000);
        assert_eq!(config.keep_snapshots, 5);
        assert!(config.compress_snapshots);
    }
}
