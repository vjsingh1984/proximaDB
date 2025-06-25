// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Compaction Module for Filestore Backend
//!
//! This module handles the compaction process for the filestore metadata backend.
//! It's responsible for:
//! - Merging incremental operations into snapshots
//! - Archiving old snapshots and incremental logs
//! - Cleaning up obsolete files
//! - Maintaining the last N snapshots for recovery
//!
//! The compaction process is atomic and blocks all API operations during execution.

use anyhow::Result;
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn, debug};

use crate::storage::filesystem::{FileSystem, FilesystemFactory};
use crate::storage::metadata::backends::filestore_backend::{
    CollectionRecord, IncrementalOperation, OperationType,
    COLLECTION_AVRO_SCHEMA,
};

// NOTE: Using unified CompactionConfig from unified_types.rs
// This specific metadata compaction config extends the base config
#[derive(Debug, Clone)]
pub struct MetadataCompactionConfig {
    /// Base compaction configuration
    pub base: crate::core::CompactionConfig,
    /// Maximum number of incremental operations before triggering compaction
    pub max_incremental_operations: usize,
    /// Maximum size of incremental logs before triggering compaction (bytes)
    pub max_incremental_size_bytes: usize,
    /// Number of snapshots to keep in archive
    pub keep_snapshots: usize,
    /// Enable compression for snapshots
    pub compress_snapshots: bool,
}

impl Default for MetadataCompactionConfig {
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

/// Compaction statistics
#[derive(Debug, Default, Clone)]
pub struct CompactionStats {
    pub last_compaction_time: Option<chrono::DateTime<chrono::Utc>>,
    pub total_compactions: u64,
    pub operations_compacted: u64,
    pub snapshots_created: u64,
    pub archives_created: u64,
    pub bytes_compacted: u64,
    pub current_incremental_count: usize,
    pub current_incremental_size: usize,
}

/// Compaction manager for filestore backend
pub struct FilestoreCompactionManager {
    config: MetadataCompactionConfig,
    filesystem: Arc<FilesystemFactory>,
    filestore_url: String,
    metadata_path: PathBuf,
    stats: CompactionStats,
}

impl FilestoreCompactionManager {
    /// Create new compaction manager
    pub fn new(
        config: MetadataCompactionConfig,
        filesystem: Arc<FilesystemFactory>,
        filestore_url: String,
    ) -> Self {
        Self {
            config,
            filesystem,
            filestore_url,
            metadata_path: PathBuf::from("metadata"),
            stats: CompactionStats::default(),
        }
    }

    /// Check if compaction is needed
    pub async fn needs_compaction(&self) -> Result<bool> {
        if !self.config.base.enable_background_compaction {
            return Ok(false);
        }

        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;
        let incremental_dir = self.metadata_path.join("incremental");
        
        let mut count = 0;
        let mut total_size = 0;

        if let Ok(entries) = fs.list(&incremental_dir.to_string_lossy()).await {
            for entry in entries {
                if entry.name.starts_with("op_") && entry.name.ends_with(".avro") {
                    count += 1;
                    total_size += entry.metadata.size as usize;
                }
            }
        }

        Ok(count >= self.config.max_incremental_operations || 
           total_size >= self.config.max_incremental_size_bytes)
    }

    /// Perform compaction - merge incremental operations into new snapshot
    pub async fn compact(&mut self) -> Result<CompactionResult> {
        info!("🗜️ Starting filestore compaction");
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
        self.stats.last_compaction_time = Some(Utc::now());
        self.stats.total_compactions += 1;
        self.stats.operations_compacted += ops_count as u64;
        self.stats.snapshots_created += 1;
        self.stats.archives_created += 1;
        self.stats.bytes_compacted += ops_size as u64;
        self.stats.current_incremental_count = 0;
        self.stats.current_incremental_size = 0;

        let result = CompactionResult {
            duration: start_time.elapsed(),
            initial_collections: initial_count,
            final_collections: memtable.len(),
            operations_compacted: ops_count,
            bytes_compacted: ops_size,
            archive_path: Some(archive_path),
        };

        info!("✅ Compaction completed: {} operations in {:?}", 
              ops_count, start_time.elapsed());

        Ok(result)
    }

    /// Load current snapshot
    async fn load_current_snapshot(&self, fs: &dyn FileSystem) -> Result<BTreeMap<String, CollectionRecord>> {
        let snapshot_path = self.metadata_path.join("snapshots/current_collections.avro");
        let mut memtable = BTreeMap::new();

        if fs.exists(&snapshot_path.to_string_lossy()).await? {
            debug!("Loading current snapshot");
            let data = fs.read(&snapshot_path.to_string_lossy()).await?;
            
            let reader = apache_avro::Reader::new(&data[..])?;
            for value in reader {
                let record: CollectionRecord = apache_avro::from_value(&value?)?;
                memtable.insert(record.uuid.clone(), record);
            }
            
            debug!("Loaded {} collections from snapshot", memtable.len());
        }

        Ok(memtable)
    }

    /// Apply incremental operations to memtable
    async fn apply_incremental_operations(
        &self,
        fs: &dyn FileSystem,
        memtable: &mut BTreeMap<String, CollectionRecord>,
    ) -> Result<(usize, usize)> {
        let incremental_dir = self.metadata_path.join("incremental");
        let mut operations = Vec::new();
        let mut total_size = 0;

        // Read all incremental operation files
        if let Ok(entries) = fs.list(&incremental_dir.to_string_lossy()).await {
            let mut op_files: Vec<_> = entries.into_iter()
                .filter(|e| e.name.starts_with("op_") && e.name.ends_with(".avro"))
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
                OperationType::Insert | OperationType::Update => {
                    if let Some(ref record) = op.collection_data {
                        memtable.insert(record.uuid.clone(), record.clone());
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
                "collection_data" => {
                    match field_value {
                        apache_avro::types::Value::String(s) => {
                            collection_data_json = Some(s);
                        }
                        apache_avro::types::Value::Null => {
                            collection_data_json = None;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        let op_type = match op_type_str.as_str() {
            "Insert" => OperationType::Insert,
            "Update" => OperationType::Update,
            "Delete" => OperationType::Delete,
            _ => return Err(anyhow::anyhow!("Invalid operation type: {}", op_type_str)),
        };

        let collection_data = collection_data_json
            .and_then(|json| serde_json::from_str::<CollectionRecord>(&json).ok());

        Ok(IncrementalOperation {
            operation_type: op_type,
            sequence_number: sequence_number as u64,
            timestamp,
            collection_id,
            collection_data,
        })
    }

    /// Create new snapshot from memtable
    async fn create_new_snapshot(
        &self,
        fs: &dyn FileSystem,
        memtable: &BTreeMap<String, CollectionRecord>,
    ) -> Result<()> {
        let snapshot_path = self.metadata_path.join("snapshots/current_collections.avro");
        let temp_path = self.metadata_path.join("snapshots/current_collections.avro.tmp");

        info!("Creating new snapshot with {} collections", memtable.len());

        // Parse schema
        let schema = apache_avro::Schema::parse_str(COLLECTION_AVRO_SCHEMA)?;
        
        // Create writer with compression
        let codec = if self.config.compress_snapshots {
            apache_avro::Codec::Deflate
        } else {
            apache_avro::Codec::Null
        };
        
        let mut writer = apache_avro::Writer::with_codec(&schema, Vec::new(), codec);

        // Write all records
        for record in memtable.values() {
            writer.append_ser(record)?;
        }

        let data = writer.into_inner()?;

        // Write atomically
        fs.write(&temp_path.to_string_lossy(), &data, None).await?;
        fs.move_file(&temp_path.to_string_lossy(), &snapshot_path.to_string_lossy()).await?;

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
        let current_snapshot = self.metadata_path.join("snapshots/current_collections.avro");
        if fs.exists(&current_snapshot.to_string_lossy()).await? {
            let archive_snapshot = archive_dir.join("snapshot_collections.avro");
            fs.copy(
                &current_snapshot.to_string_lossy(),
                &archive_snapshot.to_string_lossy(),
            ).await?;
        }

        // Move incremental files
        let incremental_dir = self.metadata_path.join("incremental");
        if let Ok(entries) = fs.list(&incremental_dir.to_string_lossy()).await {
            let archive_incremental = archive_dir.join("incremental");
            fs.create_dir(&archive_incremental.to_string_lossy()).await?;

            for entry in entries {
                if entry.name.ends_with(".avro") {
                    let src = incremental_dir.join(&entry.name);
                    let dst = archive_incremental.join(&entry.name);
                    
                    // Copy then delete
                    fs.copy(&src.to_string_lossy(), &dst.to_string_lossy()).await?;
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
            let mut archive_dirs: Vec<_> = entries.into_iter()
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

    /// Get compaction statistics
    pub fn stats(&self) -> &CompactionStats {
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
                if entry.name.starts_with("op_") && entry.name.ends_with(".avro") {
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

/// Result of a compaction operation
#[derive(Debug)]
pub struct CompactionResult {
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
    fn test_compaction_config_defaults() {
        let config = MetadataCompactionConfig::default();
        assert!(config.base.enabled);
        assert_eq!(config.max_incremental_operations, 1000);
        assert_eq!(config.keep_snapshots, 5);
        assert!(config.compress_snapshots);
    }
}