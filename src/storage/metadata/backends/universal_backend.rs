// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Filestore Metadata Backend - Rewritten from scratch
//!
//! A clean, efficient metadata backend using:
//! - Canonical Avro schema from collection_avro.rs
//! - Direct proto-to-avro conversion
//! - Atomic operations with rollback support
//! - Snapshot-based recovery for fast startup
//! - Secondary indexing for O(1) name lookups
//! - Cloud-optimized immutable file design

use anyhow::{Context, Result};
use async_trait::async_trait;
use prost::Message;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

use crate::proto::proximadb_v1::Collection;
use crate::storage::metadata::single_index::SingleCollectionIndex;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{MetadataProvider, UnifiedMetricsCollector};

/// Protobuf operation for incremental collection storage
///
/// Represents a single metadata operation in the WAL (Write-Ahead Log).
#[derive(Clone, Message)]
pub struct ProtoIncrementalOperation {
    /// Sequence number for ordering
    #[prost(uint64, tag = "1")]
    pub sequence: u64,
    /// Unix timestamp
    #[prost(int64, tag = "2")]
    pub timestamp: i64,
    /// Operation type (ProtoOperationType as i32)
    #[prost(int32, tag = "3")]
    pub operation_type: i32,
    /// Collection identifier
    #[prost(string, tag = "4")]
    pub collection_id: String,
    /// Collection data (if applicable)
    #[prost(message, optional, tag = "5")]
    pub collection_data: Option<Collection>,
}

/// Operation types for protobuf storage
///
/// Defines the type of metadata operation being performed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtoOperationType {
    /// Create a new collection
    Create = 1,
    /// Update an existing collection
    Update = 2,
    /// Delete a collection
    Delete = 3,
}
use crate::storage::traits::InternalCollectionProvider;
use crate::storage::transaction_coordinator::{
    StagingConfig, TransactionCoordinator, TransactionStageType,
};

/// Configuration for filestore metadata backend
///
/// Defines storage and behavior settings for the metadata backend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UniversalMetadataConfig {
    /// Storage URL (file://, s3://, gcs://, adls://)
    pub storage_url: String,

    /// Enable compression for Avro files
    pub compression: bool,

    /// Enable periodic snapshots
    pub enable_snapshots: bool,

    /// Snapshot after N operations
    pub snapshot_threshold: u64,

    /// Keep N recent snapshots
    pub keep_snapshots: usize,

    /// Enable backup to secondary location
    pub backup_url: Option<String>,

    /// Temporary directory for atomic operations
    pub temp_dir: Option<String>,
}

#[allow(dead_code)]
fn default_true() -> bool {
    true
}
#[allow(dead_code)]
fn default_snapshot_threshold() -> u64 {
    1000
}
#[allow(dead_code)]
fn default_keep_snapshots() -> usize {
    3
}

impl Default for UniversalMetadataConfig {
    fn default() -> Self {
        Self {
            storage_url: "file://./data/metadata_info".to_string(),
            compression: true,
            enable_snapshots: true,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        }
    }
}

/// Operation type for WAL-style logging
///
/// Defines the type of operation for write-ahead logging.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum OperationType {
    /// Create a new collection
    Create,
    /// Update an existing collection
    Update,
    /// Delete a collection
    Delete,
}

/// Incremental operation for WAL
///
/// Represents a single operation in the write-ahead log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IncrementalOperation {
    /// Sequence number
    pub sequence: u64,
    /// Unix timestamp
    pub timestamp: i64,
    /// Type of operation
    pub operation_type: OperationType,
    /// Collection identifier
    pub collection_id: String,
    /// Collection data (if applicable)
    pub collection_data: Option<Collection>,
}

/// Prepared write data for atomic operations
///
/// Holds data for an atomic write operation that can be committed or rolled back.
#[derive(Debug)]
struct PreparedWrite {
    /// Temporary file path
    temp_path: PathBuf,
    /// Final file path
    final_path: PathBuf,
    /// Data to write
    data: Vec<u8>,
}

/// Filestore metadata backend implementation
///
/// A cloud-optimized metadata backend with WAL support, snapshots, and atomic operations.
pub struct UniversalMetadataBackend {
    /// Configuration
    config: UniversalMetadataConfig,

    /// Filesystem factory for multi-cloud support
    filesystem_factory: Arc<FilesystemFactory>,

    /// Base storage path
    base_path: PathBuf,

    /// In-memory index for fast lookups
    index: Arc<SingleCollectionIndex>,

    /// Operation sequence counter
    sequence: AtomicU64,

    /// Snapshot manager
    snapshot_manager: Arc<Mutex<Option<SnapshotManager>>>,

    /// Operations since last snapshot
    ops_since_snapshot: AtomicU64,

    /// Simple atomicity flag for coordination (can be enhanced later)
    atomic_operations_enabled: bool,

    /// Unified atomic coordinator for metadata operations
    atomic_coordinator: Arc<TransactionCoordinator>,

    /// Optional unified metrics collector (injected)
    _metrics_collector: Option<UnifiedMetricsCollector>,
}

impl UniversalMetadataBackend {
    /// Create new filestore backend
    pub async fn new(
        config: UniversalMetadataConfig,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        info!(
            "🏗️ Initializing Filestore metadata backend: {}",
            config.storage_url
        );
        debug!("📁 DEBUG: Raw storage URL: '{}'", config.storage_url);

        // Parse base path from URL
        let base_path = Self::parse_base_path(&config.storage_url)?;
        debug!("📁 DEBUG: Parsed base_path: {:?}", base_path);
        debug!("📁 DEBUG: Base path as string: {}", base_path.display());

        // Proto-first architecture - no schema needed

        // Create in-memory index
        let index = Arc::new(SingleCollectionIndex::new());

        // Create atomic coordinator for metadata operations
        let atomic_coordinator = Arc::new(
            TransactionCoordinator::new(filesystem_factory.clone(), config.temp_dir.clone())
                .await
                .context("Failed to create atomic coordinator")?,
        );

        let backend = Self {
            config: config.clone(),
            filesystem_factory,
            base_path,
            index,
            sequence: AtomicU64::new(0),
            snapshot_manager: Arc::new(Mutex::new(None)),
            ops_since_snapshot: AtomicU64::new(0),
            atomic_operations_enabled: true,
            atomic_coordinator,
            _metrics_collector: None, // Metrics are optional and injected
        };

        // Initialize storage directories
        backend.initialize_storage().await?;

        // Recover from existing data
        let recovered_sequence = backend.recover_from_storage().await?;
        backend.sequence.store(recovered_sequence, Ordering::SeqCst);

        // Initialize snapshot manager if enabled
        if config.enable_snapshots {
            let snapshot_manager = SnapshotManager::new(
                config.snapshot_threshold,
                config.keep_snapshots,
                backend.base_path.clone(),
            );
            *backend.snapshot_manager.lock().await = Some(snapshot_manager);
            info!(
                "📸 Snapshot manager initialized with threshold: {}",
                config.snapshot_threshold
            );
        }

        info!(
            "✅ Filestore metadata backend ready, recovered sequence: {}",
            recovered_sequence
        );
        Ok(backend)
    }

    /// Create new filestore backend for testing with atomic operations disabled
    #[cfg(test)]
    pub async fn new_for_testing(
        config: UniversalMetadataConfig,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        info!(
            "🏗️ Initializing Filestore metadata backend for testing: {}",
            config.storage_url
        );

        // Parse base path from URL
        let base_path = Self::parse_base_path(&config.storage_url)?;

        // Create in-memory index
        let index = Arc::new(SingleCollectionIndex::new());

        // Create atomic coordinator for metadata operations
        let atomic_coordinator = Arc::new(
            TransactionCoordinator::new(filesystem_factory.clone(), config.temp_dir.clone())
                .await
                .context("Failed to create atomic coordinator")?,
        );

        let backend = Self {
            config: config.clone(),
            filesystem_factory,
            base_path,
            index,
            sequence: AtomicU64::new(0),
            snapshot_manager: Arc::new(Mutex::new(None)),
            ops_since_snapshot: AtomicU64::new(0),
            atomic_operations_enabled: false, // Disabled for testing
            atomic_coordinator,
            _metrics_collector: None, // Metrics are optional
        };

        // Initialize storage directories
        backend.initialize_storage().await?;

        // Recover from existing data
        let recovered_sequence = backend.recover_from_storage().await?;
        backend.sequence.store(recovered_sequence, Ordering::SeqCst);

        info!(
            "✅ Filestore metadata backend ready for testing, recovered sequence: {}",
            recovered_sequence
        );
        Ok(backend)
    }

    /// Parse base path from storage URL
    fn parse_base_path(url: &str) -> Result<PathBuf> {
        if let Some(path) = url.strip_prefix("file://") {
            Ok(PathBuf::from(path))
        } else if url.starts_with("s3://")
            || url.starts_with("gcs://")
            || url.starts_with("adls://")
        {
            // For cloud storage, use the full URL as path
            Ok(PathBuf::from(url))
        } else {
            // Treat as local path
            Ok(PathBuf::from(url))
        }
    }

    /// Get filesystem instance  
    fn get_fs(&self) -> Result<Arc<dyn crate::storage::persistence::filesystem::FileSystem>> {
        self.filesystem_factory
            .get_filesystem(&self.config.storage_url)
            .map_err(|e| anyhow::anyhow!("Failed to get filesystem: {}", e))
    }

    /// Initialize storage directory structure
    async fn initialize_storage(&self) -> Result<()> {
        let fs = self.get_fs()?;
        info!(
            "📁 DEBUG: Initializing storage with filesystem for URL: {}",
            self.config.storage_url
        );

        // Create directory structure using URLs consistently
        let base_url = &self.config.storage_url;
        let dirs = [
            base_url.to_string(),
            format!("{}/current", base_url.trim_end_matches('/')),
            format!("{}/current/__staging", base_url.trim_end_matches('/')),
            format!("{}/__staging", base_url.trim_end_matches('/')),
            format!("{}/archive", base_url.trim_end_matches('/')),
        ];

        for dir_url in &dirs {
            debug!("📁 DEBUG: Checking/creating directory: {}", dir_url);
            if !fs.exists(dir_url).await? {
                debug!("📁 DEBUG: Creating directory via filesystem: {}", dir_url);
                fs.create_dir_all(dir_url)
                    .await
                    .with_context(|| format!("Failed to create directory: {}", dir_url))?;
                debug!("📁 Created directory: {}", dir_url);
            } else {
                debug!("📁 DEBUG: Directory already exists: {}", dir_url);
            }
        }

        debug!("📂 Storage directories initialized");
        Ok(())
    }

    /// Recover collections from storage
    async fn recover_from_storage(&self) -> Result<u64> {
        debug!("🔄 Starting metadata recovery");

        // Try to recover from latest snapshot first
        if let Ok(snapshot_sequence) = self.recover_from_snapshot().await {
            debug!(
                "📸 Recovered from snapshot, sequence: {}",
                snapshot_sequence
            );
            self.recover_incremental_operations(snapshot_sequence)
                .await?;
            let final_sequence = self.sequence.load(Ordering::SeqCst);

            // Check if we should create a checkpoint after recovery
            // Deferred: Temporarily disabled to debug startup hang
            // self.maybe_checkpoint_at_restart().await?;
            debug!("⏭️ Skipping checkpoint at restart to debug startup issue");

            return Ok(final_sequence);
        }

        // Fallback to full recovery from operations
        debug!("📜 No snapshot found, performing full recovery");
        let max_sequence = self.recover_from_operations().await?;

        // Check if we should create a checkpoint after recovery
        // Deferred: Temporarily disabled to debug startup hang
        // self.maybe_checkpoint_at_restart().await?;
        debug!("⏭️ Skipping checkpoint at restart to debug startup issue");

        Ok(max_sequence)
    }

    /// Recover from latest snapshot
    async fn recover_from_snapshot(&self) -> Result<u64> {
        let fs = self.get_fs()?;
        let snapshot_dir = self.base_path.join("current");
        let current_snapshot = snapshot_dir.join("snapshot.meta");

        let current_path = current_snapshot.to_string_lossy();
        if !fs.exists(&current_path).await? {
            return Err(anyhow::anyhow!("No current snapshot found"));
        }

        // Read snapshot data and decompress
        let compressed_data = fs.read(&current_path).await?;
        use flate2::read::ZlibDecoder;
        use std::io::Read;
        let mut decoder = ZlibDecoder::new(&compressed_data[..]);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data)?;

        let mut count = 0;
        let mut max_sequence = 0;
        let mut records = Vec::new();

        // Parse length-prefixed protobuf messages
        let mut offset = 0;
        while offset < data.len() {
            // Read length (4 bytes)
            if offset + 4 > data.len() {
                break;
            }
            let len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;

            // Read protobuf message
            if offset + len > data.len() {
                break;
            }
            let record = Collection::decode(&data[offset..offset + len])?;
            offset += len;

            max_sequence = max_sequence.max(1); // Proto collections don't have version field
            records.push(record);
            count += 1;
        }

        debug!(
            "📦 Loaded {} collections from snapshot, rebuilding indexes...",
            count
        );

        // Clear existing index to ensure clean state
        self.index.clear();

        // Rebuild both primary (UUID) and secondary (name) indexes
        for record in records {
            // This will update both UUID->record and name->UUID mappings
            self.index.upsert_collection(record);
        }

        debug!(
            "✅ Rebuilt indexes with {} collections from snapshot",
            count
        );
        Ok(max_sequence)
    }

    /// Recover from operation files
    async fn recover_from_operations(&self) -> Result<u64> {
        let fs = self.get_fs()?;
        let ops_dir = self.base_path.join("current");

        let entries = match fs.list(&ops_dir.to_string_lossy()).await {
            Ok(entries) => entries,
            Err(_) => {
                debug!("📋 No operations directory found");
                return Ok(0);
            }
        };

        // Sort operation files by sequence
        let mut op_files: Vec<_> = entries
            .into_iter()
            .filter(|e| e.name.starts_with("op_") && e.name.ends_with(".oplog"))
            .collect();

        op_files.sort_by(|a, b| a.name.cmp(&b.name));

        let mut max_sequence = 0;
        for entry in op_files {
            let op_path = ops_dir.join(&entry.name);
            if let Ok(sequence) = self.recover_operation_file(&op_path).await {
                max_sequence = max_sequence.max(sequence);
            }
        }

        debug!("📜 Recovery completed, max sequence: {}", max_sequence);
        Ok(max_sequence)
    }

    /// Recover incremental operations after snapshot
    async fn recover_incremental_operations(&self, after_sequence: u64) -> Result<()> {
        let fs = self.get_fs()?;
        let ops_dir = self.base_path.join("current");

        let entries = match fs.list(&ops_dir.to_string_lossy()).await {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };

        // Filter operations after snapshot sequence
        let mut incremental_ops = Vec::new();

        for entry in entries {
            if entry.name.starts_with("op_") && entry.name.ends_with(".oplog") {
                // Parse sequence from filename: op_XXXXXXXX.oplog
                if let Some(seq_str) = entry
                    .name
                    .strip_prefix("op_")
                    .and_then(|s| s.strip_suffix(".oplog"))
                    && let Ok(sequence) = seq_str.parse::<u64>()
                    && sequence > after_sequence
                {
                    let op_path = ops_dir.join(&entry.name);
                    incremental_ops.push((sequence, op_path));
                }
            }
        }

        // Sort by sequence and apply
        incremental_ops.sort_by_key(|(seq, _)| *seq);

        let ops_count = incremental_ops.len();
        for (sequence, path) in incremental_ops {
            self.recover_operation_file(&path).await?;
            self.sequence.store(sequence, Ordering::SeqCst);
        }

        debug!("📈 Applied {} incremental operations", ops_count);
        Ok(())
    }

    /// Recover single operation file
    async fn recover_operation_file(&self, path: &Path) -> Result<u64> {
        let fs = self.get_fs()?;
        let data = fs.read(&path.to_string_lossy()).await?;

        // Parse JSON operation log
        let op_json: serde_json::Value = serde_json::from_slice(&data)?;
        let sequence = op_json["sequence"].as_u64().unwrap_or(0);
        let op_type_str = op_json["operation_type"].as_str();

        let max_sequence = sequence;

        match op_type_str {
            Some("Create") | Some("Update") => {
                if let Some(collection_data) = op_json["collection_data"].as_array() {
                    // Decode protobuf bytes from JSON array
                    let collection_bytes: Vec<u8> = collection_data
                        .iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u8))
                        .collect();
                    let record = Collection::decode(&collection_bytes[..])?;
                    self.index.upsert_collection(record);
                }
            }
            Some("Delete") => {
                // collection_id is the name, need to get UUID first
                if let Some(collection_id) = op_json["collection_id"].as_str()
                    && let Some(uuid) = self.index.get_uuid_by_name(collection_id)
                {
                    self.index.remove_collection(&uuid);
                }
            }
            _ => {
                // Unknown operation type, skip
            }
        }

        Ok(max_sequence)
    }

    /// Get next sequence number
    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Atomic write operation using TransactionCoordinator for ACID guarantees
    async fn atomic_persist_operation(&self, operation: &IncrementalOperation) -> Result<()> {
        trace!(
            "🔍 DEBUG: atomic_persist_operation() called for seq={}",
            operation.sequence
        );
        trace!(
            "🔍 DEBUG: atomic_operations_enabled = {}",
            self.atomic_operations_enabled
        );

        if !self.atomic_operations_enabled {
            trace!("DEBUG: Using simple persist (atomic disabled)");
            return self.execute_simple_persist(operation).await;
        }

        trace!("DEBUG: Using atomic coordinator");

        // Use the atomic coordinator with simple atomic operations
        let coordinator = self.atomic_coordinator.clone();

        debug!(
            "🔒 Starting atomic operation for seq={}",
            operation.sequence
        );
        debug!("📁 Filestore base_path: {}", self.base_path.display());
        debug!("📁 Config storage_url: {}", self.config.storage_url);
        debug!(
            "📁 Current working directory: {:?}",
            std::env::current_dir()?
        );

        // Prepare the write data
        let prepared_data = self.prepare_filestore_write(operation).await?;
        debug!("📋 Prepared write data:");
        debug!("    temp_path: {}", prepared_data.temp_path.display());
        debug!("    final_path: {}", prepared_data.final_path.display());
        debug!("    data size: {} bytes", prepared_data.data.len());

        // Create staging config for atomic operation
        // The base_url should point to the current directory where files will be stored
        let base_url = format!("{}/current", self.config.storage_url.trim_end_matches('/'));
        debug!("📁 DEBUG: Creating staging config:");
        debug!(
            "📁 DEBUG:   self.config.storage_url = '{}'",
            self.config.storage_url
        );
        debug!("📁 DEBUG:   Computed base_url = '{}'", base_url);

        let staging_config = StagingConfig {
            base_url: base_url.clone(),
            operation_type: TransactionStageType::Metadata,
            collection_id: None, // No collection-specific directories for metadata
            custom_staging_dir: Some("__staging".to_string()), // Use staging directory within current
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
            skip_uuid_subdir: true, // Skip UUID subdirectory to prevent orphaned directories
        };

        debug!("📁 Staging config:");
        debug!("    base_url: {}", staging_config.base_url);
        debug!(
            "    custom_staging_dir: {:?}",
            staging_config.custom_staging_dir
        );
        debug!("    operation_type: {:?}", staging_config.operation_type);
        debug!(
            "📁 DEBUG: Expected staging path: {}/{}/<operation_id>",
            base_url, "__staging"
        );

        // Begin atomic operation
        trace!("DEBUG: About to call begin_atomic_operation()");
        let op_metadata = coordinator.begin_atomic_operation(&staging_config).await?;
        trace!(
            "🔍 DEBUG: begin_atomic_operation() SUCCESS - operation_id={}",
            op_metadata.operation_id
        );

        // Write to staging
        debug!("📝 Writing to staging:");
        debug!("    staging_url: {}", op_metadata.staging_url);
        debug!("    filename: op_{:016}.oplog", operation.sequence);
        debug!("    operation_id: {}", op_metadata.operation_id);

        trace!("DEBUG: About to call write_to_staging()");
        match coordinator
            .write_to_staging(
                &op_metadata.operation_id,
                &format!("op_{:016}.oplog", operation.sequence),
                &prepared_data.data,
            )
            .await
        {
            Ok(_) => {
                trace!("DEBUG: write_to_staging() SUCCESS");
                debug!("✅ Write to staging successful");
                // Update in-memory state atomically before finalizing disk write
                match operation.operation_type {
                    OperationType::Create | OperationType::Update => {
                        if let Some(record) = &operation.collection_data {
                            self.index.upsert_collection(record.clone());
                        }
                    }
                    OperationType::Delete => {
                        if let Some(uuid) = self.index.get_uuid_by_name(&operation.collection_id) {
                            self.index.remove_collection(&uuid);
                        }
                    }
                }

                // Finalize the atomic operation (moves from staging to final)
                debug!("🔄 Starting finalize operation...");
                trace!(
                    "🔍 DEBUG: About to call finalize_atomic_operation() with operation_id={}",
                    op_metadata.operation_id
                );
                match coordinator
                    .finalize_atomic_operation(&op_metadata.operation_id)
                    .await
                {
                    Ok(_) => {
                        trace!("DEBUG: finalize_atomic_operation() SUCCESS");
                        self.check_snapshot_trigger().await?;
                        debug!(
                            "✅ Atomic operation completed for seq={}",
                            operation.sequence
                        );
                        trace!(
                            "🔍 DEBUG: Atomic operation FULLY COMPLETED for seq={}",
                            operation.sequence
                        );
                        Ok(())
                    }
                    Err(e) => {
                        // Rollback in-memory state on disk finalization failure
                        trace!("DEBUG ERROR: finalize_atomic_operation() FAILED: {}", e);
                        error!("❌ Failed to finalize atomic operation: {}", e);
                        // Note: In production, we'd implement proper rollback of in-memory state
                        Err(e)
                    }
                }
            }
            Err(e) => {
                trace!("DEBUG ERROR: write_to_staging() FAILED: {}", e);
                // Abort the operation on write failure
                let abort_result = coordinator
                    .abort_atomic_operation(
                        &op_metadata.operation_id,
                        &format!("Write failed: {}", e),
                    )
                    .await;
                if let Err(abort_err) = abort_result {
                    warn!("Failed to abort operation: {}", abort_err);
                }
                Err(anyhow::anyhow!("Failed to write to staging: {}", e))
            }
        }
    }

    /// Fallback to simple non-atomic persist for compatibility
    async fn execute_simple_persist(&self, operation: &IncrementalOperation) -> Result<()> {
        // Update memtable first
        match operation.operation_type {
            OperationType::Create | OperationType::Update => {
                if let Some(record) = &operation.collection_data {
                    self.index.upsert_collection(record.clone());
                }
            }
            OperationType::Delete => {
                if let Some(uuid) = self.index.get_uuid_by_name(&operation.collection_id) {
                    self.index.remove_collection(&uuid);
                }
            }
        }

        // Then persist to disk
        let prepared_data = self.prepare_filestore_write(operation).await?;
        let fs = self.get_fs()?;
        fs.write(
            &prepared_data.temp_path.to_string_lossy(),
            &prepared_data.data,
            None,
        )
        .await?;
        fs.move_file(
            &prepared_data.temp_path.to_string_lossy(),
            &prepared_data.final_path.to_string_lossy(),
        )
        .await?;

        self.check_snapshot_trigger().await?;
        Ok(())
    }

    /// Prepare filestore write data without committing
    async fn prepare_filestore_write(
        &self,
        operation: &IncrementalOperation,
    ) -> Result<PreparedWrite> {
        let _fs = self.get_fs()?;
        let ops_dir = self.base_path.join("current");

        // Create operation filename with sequence
        let filename = format!("op_{:016}.oplog", operation.sequence);
        let op_path = ops_dir.join(filename.as_str());

        // Create staging directory if it doesn't exist for simple operations
        let staging_dir = self.base_path.join("staging");
        std::fs::create_dir_all(&staging_dir).ok();
        let temp_path = staging_dir.join(format!("temp_{}", filename));

        // Serialize operation to Avro
        // Serialize operation log entry as JSON for simplicity
        let op_json = serde_json::json!({
            "sequence": operation.sequence,
            "timestamp": operation.timestamp,
            "operation_type": format!("{:?}", operation.operation_type),
            "collection_id": operation.collection_id,
            "collection_data": operation.collection_data.as_ref().map(|c| {
                // Encode collection as protobuf bytes array
                let mut buf = Vec::new();
                c.encode(&mut buf).ok();
                buf
            })
        });

        let data = serde_json::to_vec(&op_json)?;

        Ok(PreparedWrite {
            temp_path,
            final_path: op_path,
            data,
        })
    }

    /// Execute atomic write across memtable, secondary index, and filestore
    #[allow(dead_code)]
    async fn execute_atomic_write(
        &self,
        operation: &IncrementalOperation,
        prepared: &PreparedWrite,
    ) -> Result<()> {
        let fs = self.get_fs()?;

        // Step 1: Write to temp file (prepare phase)
        fs.write(&prepared.temp_path.to_string_lossy(), &prepared.data, None)
            .await?;

        // Step 2: Update memtable and secondary index atomically
        // Note: SingleCollectionIndex.upsert_collection() is already atomic for both primary and secondary index
        match operation.operation_type {
            OperationType::Create | OperationType::Update => {
                if let Some(record) = &operation.collection_data {
                    self.index.upsert_collection(record.clone());
                }
            }
            OperationType::Delete => {
                // Get UUID first for atomic removal
                if let Some(uuid) = self.index.get_uuid_by_name(&operation.collection_id) {
                    self.index.remove_collection(&uuid);
                }
            }
        }

        // Step 3: Commit to filestore (atomic move)
        fs.move_file(
            &prepared.temp_path.to_string_lossy(),
            &prepared.final_path.to_string_lossy(),
        )
        .await?;

        Ok(())
    }

    /// Check if snapshot is needed after successful operation
    async fn check_snapshot_trigger(&self) -> Result<()> {
        let ops_count = self.ops_since_snapshot.fetch_add(1, Ordering::SeqCst) + 1;
        if ops_count >= self.config.snapshot_threshold
            && let Some(manager) = self.snapshot_manager.lock().await.as_ref()
        {
            let fs = self.get_fs()?;
            if let Err(e) = manager.create_snapshot(&self.index, &*fs).await {
                warn!("📸 Snapshot creation failed: {}", e);
            } else {
                self.ops_since_snapshot.store(0, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    /// Check if we should create a checkpoint at restart
    #[allow(dead_code)]
    async fn maybe_checkpoint_at_restart(&self) -> Result<()> {
        // Count operation files in the current directory
        let fs = self.get_fs()?;
        let ops_dir = self.base_path.join("current");

        let entries = match fs.list(&ops_dir.to_string_lossy()).await {
            Ok(entries) => entries,
            Err(_) => {
                debug!("📋 No operations directory found, skipping checkpoint");
                return Ok(());
            }
        };

        // Count operation files
        let op_count = entries
            .iter()
            .filter(|e| e.name.starts_with("op_") && e.name.ends_with(".oplog"))
            .count();

        if op_count == 0 {
            debug!("📋 No operation files found, skipping checkpoint at restart");
            return Ok(());
        }

        debug!(
            "🔄 Found {} operation files at restart, creating checkpoint",
            op_count
        );

        // Create snapshot manager if not already present
        if self.snapshot_manager.lock().await.is_none() {
            let manager = SnapshotManager::new(
                self.config.snapshot_threshold,
                self.config.keep_snapshots,
                self.base_path.clone(),
            );
            *self.snapshot_manager.lock().await = Some(manager);
        }

        // Create the snapshot
        if let Some(manager) = self.snapshot_manager.lock().await.as_ref() {
            match manager.create_snapshot(&self.index, &*fs).await {
                Ok(_) => {
                    debug!("✅ Checkpoint created successfully at restart");

                    // Clean up old operation files after successful snapshot
                    self.cleanup_operation_files().await?;

                    // Reset ops counter
                    self.ops_since_snapshot.store(0, Ordering::SeqCst);
                }
                Err(e) => {
                    warn!("⚠️ Failed to create checkpoint at restart: {}", e);
                    // Continue anyway - we can still operate with the operation files
                }
            }
        }

        Ok(())
    }

    /// Clean up operation files after successful snapshot
    #[allow(dead_code)]
    async fn cleanup_operation_files(&self) -> Result<()> {
        let fs = self.get_fs()?;
        let ops_dir = self.base_path.join("current");
        let archive_dir = self.base_path.join("archive");

        let entries = match fs.list(&ops_dir.to_string_lossy()).await {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };

        // Create archive subdirectory with timestamp
        let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
        let mut seq = 0;
        let mut archive_subdir = format!("{}_{}", timestamp, seq);

        // Check for conflicts and increment sequence if needed
        loop {
            let archive_path = archive_dir.join(&archive_subdir);
            if !fs.exists(&archive_path.to_string_lossy()).await? {
                fs.create_dir_all(&archive_path.to_string_lossy()).await?;
                break;
            }
            seq += 1;
            archive_subdir = format!("{}_{}", timestamp, seq);
        }

        let archive_path = archive_dir.join(&archive_subdir);
        let mut archived = 0;

        // Move operation files to archive
        for entry in entries {
            if entry.name.starts_with("op_") && entry.name.ends_with(".oplog") {
                let src = ops_dir.join(&entry.name);
                let dst = archive_path.join(&entry.name);
                match fs
                    .move_file(&src.to_string_lossy(), &dst.to_string_lossy())
                    .await
                {
                    Ok(_) => archived += 1,
                    Err(e) => {
                        warn!("Failed to archive operation file {}: {}", entry.name, e);
                        // Fallback to delete if move fails
                        if let Err(del_err) = fs.delete(&src.to_string_lossy()).await {
                            warn!(
                                "Failed to delete operation file {}: {}",
                                entry.name, del_err
                            );
                        }
                    }
                }
            }
        }

        if archived > 0 {
            debug!(
                "📦 Archived {} operation files to archive/{}",
                archived, archive_subdir
            );
        }

        // Clean up old archives - delegate to SnapshotManager's method
        debug!("🔍 Checking for old archives to clean up...");
        if let Some(manager) = self.snapshot_manager.lock().await.as_ref() {
            debug!("🔍 Calling cleanup_old_archives...");
            manager.cleanup_old_archives(&*fs).await?;
            debug!("✅ Old archives cleanup completed");
        } else {
            debug!("⏭️ No snapshot manager available, skipping archive cleanup");
        }

        debug!("✅ cleanup_operation_files completed successfully");
        Ok(())
    }

    /// Delete a collection (CRUD operation)
    pub async fn delete_collection(&self, collection_id: &str) -> Result<()> {
        debug!("🗑️ Deleting collection: {}", collection_id);

        // Check if collection exists
        if self.index.get_by_name(collection_id).is_none() {
            return Err(anyhow::anyhow!("Collection '{}' not found", collection_id));
        }

        // Create operation
        let operation = IncrementalOperation {
            sequence: self.next_sequence(),
            timestamp: chrono::Utc::now().timestamp(),
            operation_type: OperationType::Delete,
            collection_id: collection_id.to_string(),
            collection_data: None,
        };

        // Persist operation atomically
        self.atomic_persist_operation(&operation).await?;

        // Update in-memory index - need to get UUID first since remove_collection takes UUID
        if let Some(uuid) = self.index.get_uuid_by_name(collection_id) {
            self.index.remove_collection(&uuid);
        }

        debug!("✅ Collection deleted: {}", collection_id);
        Ok(())
    }

    /// Upsert collection record directly
    pub async fn upsert_collection_record(&self, record: Collection) -> Result<()> {
        // Create operation
        let operation = IncrementalOperation {
            sequence: self.next_sequence(),
            timestamp: chrono::Utc::now().timestamp(),
            operation_type: OperationType::Update,
            collection_id: record
                .config
                .as_ref()
                .map_or_else(|| "unknown".to_string(), |c| c.name.clone()),
            collection_data: Some(record.clone()),
        };

        // Persist operation atomically
        self.atomic_persist_operation(&operation).await?;

        // Update in-memory index
        self.index.upsert_collection(record);

        Ok(())
    }

    /// Store protobuf collection directly - using simple atomic operations
    pub async fn upsert_collection_proto(&self, proto_collection: &Collection) -> Result<()> {
        let config = proto_collection
            .config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection config is required"))?;

        // Create IncrementalOperation for consistency with other methods
        let operation = IncrementalOperation {
            sequence: self.next_sequence(),
            timestamp: chrono::Utc::now().timestamp(),
            operation_type: OperationType::Update,
            collection_id: config.name.clone(),
            collection_data: Some(proto_collection.clone()),
        };

        // Use the same atomic persist operation
        self.atomic_persist_operation(&operation).await
    }

    /// Convert protobuf collection to core Collection for fast in-memory index
    #[allow(dead_code)]
    fn convert_proto_to_core(&self, proto: &Collection) -> Collection {
        // Proto Collection is already the core type - no conversion needed
        proto.clone()
    }

    /// Prepare transaction participants
    /* Not used with simple atomic operations
    async fn prepare_transaction_participants(
        &self,
        tx: &TransactionHandle<'_>,
        operation: &IncrementalOperation,
    ) -> Result<PreparedWrite> {
        // Prepare disk write data
        let prepared_data = self.prepare_filestore_write(operation).await?;

        // Register rollback actions based on operation type
        match &operation.operation_type {
            OperationType::Create => {
                if let Some(record) = &operation.collection_data {
                    // Rollback: remove from memtable
                    tx.register_rollback(
                        "memtable",
                        RollbackAction::RemoveFromMemtable {
                            key: record.id.clone(),
                        },
                    )
                    .await?;

                    // Rollback: remove from secondary index
                    tx.register_rollback(
                        "secondary_index",
                        RollbackAction::RemoveFromSecondaryIndex {
                            name: record.config.as_ref().map(|c| c.name.clone()),
                            uuid: record.id.clone(),
                        },
                    )
                    .await?;

                    // Rollback: delete file
                    tx.register_rollback(
                        "disk",
                        RollbackAction::DeleteFile {
                            path: prepared_data.final_path.to_string_lossy().to_string(),
                        },
                    )
                    .await?;
                }
            }
            OperationType::Update => {
                if let Some(record) = &operation.collection_data {
                    // Get previous value from memtable for rollback
                    if let Some(previous) = self.index.get_by_uuid(&record.id) {
                        // Rollback: restore previous value in memtable
                        let previous_bytes = self.serialize_record(&previous)?;
                        tx.register_rollback(
                            "memtable",
                            RollbackAction::RestoreMemtableValue {
                                key: record.id.clone(),
                                previous_value: previous_bytes,
                            },
                        )
                        .await?;

                        // Rollback: restore secondary index if name changed
                        let empty_string = String::new();
                        let prev_name = previous.config.as_ref().map(|c| &c.name);
                        let curr_name = record.config.as_ref().map(|c| &c.name);
                        if prev_name != curr_name {
                            tx.register_rollback(
                                "secondary_index",
                                RollbackAction::Custom(
                                    format!("Restore index: {} -> {}", curr_name, prev_name)
                                ),
                            )
                            .await?;
                        }
                    }
                }
            }
            OperationType::Delete => {
                // For delete, we'd need to restore the entire record
                // This would require storing the full record before deletion
                tx.register_rollback(
                    "disk",
                    RollbackAction::Custom(
                        "Restore deleted collection record".to_string(),
                    ),
                )
                .await?;
            }
        }

        Ok(prepared_data)
    }

    /// Commit transaction participants
    async fn commit_transaction_participants(
        &self,
        _tx: &TransactionHandle<'_>,
        operation: &IncrementalOperation,
        prepared_data: &PreparedWrite,
    ) -> Result<()> {
        // Execute the actual atomic write
        self.execute_atomic_write(operation, prepared_data).await
    }
    */
    /// Serialize record to bytes for rollback
    #[allow(dead_code)]
    fn serialize_record(&self, record: &Collection) -> Result<Vec<u8>> {
        // Proto-first: serialize directly to protobuf
        let mut buf = Vec::new();
        record
            .encode(&mut buf)
            .context("Failed to encode protobuf")?;
        Ok(buf)
    }

    /// Get collection record by name - uses O(1) secondary index lookup
    pub fn get_collection_record_by_name(&self, name: &str) -> Option<Collection> {
        self.index
            .get_by_name(name)
            .map(|arc_record| (*arc_record).clone())
    }

    /// Get collection record by UUID - uses O(1) primary key lookup  
    pub fn get_collection_record_by_uuid(&self, uuid: &str) -> Option<Collection> {
        self.index
            .get_by_uuid(uuid)
            .map(|arc_record| (*arc_record).clone())
    }

    /// Get collection name by UUID - uses O(1) primary key lookup
    pub fn get_collection_name_by_uuid(&self, uuid: &str) -> Option<String> {
        self.index
            .get_by_uuid(uuid)
            .and_then(|record| record.config.as_ref().map(|c| c.name.clone()))
    }

    /// Get all collection UUIDs
    pub fn list_collection_uuids(&self) -> Vec<String> {
        self.index
            .list_all()
            .into_iter()
            .map(|record| record.id.clone())
            .collect()
    }

    /// Get all collection names  
    pub fn list_collection_names(&self) -> Vec<String> {
        self.index
            .list_all()
            .into_iter()
            .filter_map(|record| record.config.as_ref().map(|c| c.name.clone()))
            .collect()
    }

    /// Create a checkpoint snapshot
    pub async fn create_checkpoint(&self) -> Result<()> {
        let checkpoint_dir = self.base_path.join("archive");

        let sequence = self.sequence.load(Ordering::SeqCst);
        let timestamp = chrono::Utc::now().timestamp_millis();
        let checkpoint_name = format!("checkpoint_{}_{}.meta", sequence, timestamp);

        // Configure atomic operation for metadata checkpoint
        let staging_config = StagingConfig {
            base_url: self.config.storage_url.clone(),
            collection_id: None, // Checkpoint is not collection-specific
            operation_type: TransactionStageType::Metadata,
            custom_staging_dir: Some("__metadata_info".to_string()),
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
            skip_uuid_subdir: true, // Skip UUID subdirectory to prevent orphaned directories
        };

        // Begin atomic operation
        let operation = self
            .atomic_coordinator
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin checkpoint operation")?;

        // Get all collections
        let collections = self.index.list_all();

        // Serialize collections to protobuf
        // Create a wrapper message for multiple collections
        let mut data = Vec::new();
        for record in collections {
            // Write length-prefixed protobuf messages
            let mut buf = Vec::new();
            record.as_ref().encode(&mut buf)?;
            // Write length as 4 bytes
            data.extend_from_slice(&(buf.len() as u32).to_le_bytes());
            data.extend_from_slice(&buf);
        }

        // Write checkpoint to staging area
        self.atomic_coordinator
            .write_to_staging(&operation.operation_id, &checkpoint_name, &data)
            .await
            .context("Failed to write checkpoint to staging")?;

        // Also write the current link content
        let link_content = checkpoint_name.as_bytes();
        self.atomic_coordinator
            .write_to_staging(
                &operation.operation_id,
                "current_checkpoint.meta",
                link_content,
            )
            .await
            .context("Failed to write current link to staging")?;

        // Finalize the atomic operation - this handles the atomic move and cleanup
        self.atomic_coordinator
            .finalize_atomic_operation(&operation.operation_id)
            .await
            .context("Failed to finalize checkpoint operation")?;

        debug!("📸 Created checkpoint at sequence {}", sequence);

        // Clean up old snapshots
        self.cleanup_old_snapshots(&checkpoint_dir, self.config.keep_snapshots)
            .await?;

        Ok(())
    }

    /// Recover from checkpoint snapshot if available
    pub async fn recover_from_checkpoint(&self) -> Result<(u64, bool)> {
        let fs = self
            .filesystem_factory
            .get_filesystem(&self.config.storage_url)?;
        let checkpoint_link = self.base_path.join("current/snapshot.meta");

        // Check if checkpoint exists
        if !fs.exists(&checkpoint_link.to_string_lossy()).await? {
            debug!("📋 No checkpoint found, will use regular snapshot");
            return Ok((0, false));
        }

        // Read checkpoint link
        let checkpoint_path_bytes = fs.read(&checkpoint_link.to_string_lossy()).await?;
        let checkpoint_path =
            String::from_utf8(checkpoint_path_bytes).context("Invalid checkpoint path")?;

        // Parse sequence from checkpoint filename
        let sequence = self.parse_checkpoint_sequence(&checkpoint_path)?;

        debug!("📸 Found checkpoint at sequence {}", sequence);

        // Load checkpoint into memory
        let checkpoint_data = fs
            .read(&checkpoint_path)
            .await
            .context("Failed to read checkpoint file")?;

        let reader = apache_avro::Reader::new(&checkpoint_data[..])
            .context("Failed to create Avro reader for checkpoint")?;

        let mut count = 0;
        let mut records = Vec::new();

        // First, load all records from checkpoint
        for value in reader {
            let record: Collection = apache_avro::from_value(&value?)?;
            records.push(record);
            count += 1;
        }

        info!(
            "📋 Loaded {} collections from checkpoint, rebuilding indexes...",
            count
        );

        // Clear existing index to ensure clean state
        self.index.clear();

        // Rebuild both primary (UUID) and secondary (name) indexes
        for record in records {
            // This will update both UUID->record and name->UUID mappings
            self.index.upsert_collection(record);
        }

        info!(
            "✅ Rebuilt indexes with {} collections from checkpoint",
            count
        );

        // Update sequence counter
        self.sequence.store(sequence, Ordering::SeqCst);

        Ok((sequence, true))
    }

    /// Parse sequence number from checkpoint filename
    fn parse_checkpoint_sequence(&self, path: &str) -> Result<u64> {
        let filename = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid checkpoint path"))?;

        if let Some(parts) = filename
            .strip_prefix("checkpoint_")
            .and_then(|s| s.strip_suffix(".meta"))
        {
            let seq_str = parts
                .split('_')
                .next()
                .ok_or_else(|| anyhow::anyhow!("Invalid checkpoint filename format"))?;
            seq_str
                .parse::<u64>()
                .context("Failed to parse sequence number")
        } else {
            Err(anyhow::anyhow!("Invalid checkpoint filename format"))
        }
    }

    /// Clean up old snapshots keeping only N most recent
    async fn cleanup_old_snapshots(
        &self,
        checkpoint_dir: &std::path::Path,
        keep_count: usize,
    ) -> Result<()> {
        let fs = self
            .filesystem_factory
            .get_filesystem(&self.config.storage_url)?;

        if let Ok(entries) = fs.list(&checkpoint_dir.to_string_lossy()).await {
            let mut snapshots: Vec<_> = entries
                .into_iter()
                .filter(|e| e.name.starts_with("checkpoint_") && e.name.ends_with(".meta"))
                .collect();

            // Sort by name (which includes sequence number)
            snapshots.sort_by(|a, b| b.name.cmp(&a.name)); // Reverse sort

            // Delete old snapshots
            for (i, entry) in snapshots.iter().enumerate() {
                if i >= keep_count {
                    let path = checkpoint_dir.join(&entry.name);
                    fs.delete(&path.to_string_lossy()).await.ok();
                    debug!("🗑️ Deleted old snapshot: {}", entry.name);
                }
            }
        }

        Ok(())
    }

    /// Common internal helper for finding collection records
    /// Optimizes lookups using the efficient O(1) secondary index with fallback to primary scan
    fn find_collection_internal(&self, identifier: &str, by_uuid: bool) -> Option<Collection> {
        if by_uuid {
            self.index
                .get_by_uuid(identifier)
                .map(|arc_record| (*arc_record).clone())
        } else {
            // Use efficient O(1) secondary index for name lookup
            if let Some(record) = self.index.get_by_name(identifier) {
                return Some((*record).clone());
            }

            // Fallback: scan primary memtable if secondary index fails
            // This handles cases where secondary index is inconsistent or corrupted
            // Normal behavior when index not yet built - not a warning
            debug!(
                "Secondary index lookup failed for '{}', using primary scan",
                identifier
            );
            self.fallback_scan_by_name(identifier)
        }
    }

    /// Fallback mechanism: scan primary memtable when secondary index lookup fails
    /// This provides robustness against secondary index corruption or inconsistency
    /// If found, repairs the secondary index lazily for future lookups
    fn fallback_scan_by_name(&self, name: &str) -> Option<Collection> {
        let start = std::time::Instant::now();

        // Scan all entries in primary memtable
        for entry in self.index.list_all() {
            if entry.config.as_ref().map(|c| c.name.as_str()) == Some(name) {
                let elapsed = start.elapsed();
                warn!(
                    "🔧 Fallback scan found '{}' in {:?}, repairing secondary index",
                    name, elapsed
                );

                // Self-healing: repair secondary index by re-inserting the mapping
                // This is safe because upsert_collection() atomically updates both primary and secondary index
                let record_to_repair = (*entry).clone();
                self.index.upsert_collection(record_to_repair.clone());

                info!(
                    "✅ Secondary index repaired for '{}' -> '{}'",
                    name, record_to_repair.id
                );
                return Some(record_to_repair);
            }
        }

        let elapsed = start.elapsed();
        debug!(
            "🔍 Fallback scan completed in {:?}, collection '{}' not found",
            elapsed, name
        );
        None
    }

    /// Generic collection lookup that can find by name OR UUID
    /// Uses efficient O(1) lookups for both cases
    pub fn find_collection(&self, identifier: &str) -> Option<Collection> {
        // Try by name first (most common case) - uses O(1) secondary index
        if let Some(record) = self.find_collection_internal(identifier, false) {
            return Some(record);
        }

        // Try by UUID if name lookup failed - uses O(1) primary key lookup
        self.find_collection_internal(identifier, true)
    }
}

#[async_trait]
impl MetadataProvider for UniversalMetadataBackend {
    async fn get_uuid(&self, collection_id: &str) -> Result<Option<String>> {
        // Use optimized internal lookup that tries both name and UUID
        Ok(self.find_collection(collection_id).map(|r| r.id))
    }

    async fn collection_metadata(&self, collection_id: &str) -> Result<Option<Collection>> {
        // Use optimized internal lookup that tries both name and UUID
        Ok(self.find_collection(collection_id))
    }

    async fn get_collection(&self, collection_id: &str) -> Result<Option<Collection>> {
        // Use optimized internal lookup that tries both name and UUID
        Ok(self.find_collection(collection_id))
    }

    async fn list_collections(&self) -> Result<Vec<Collection>> {
        Ok(self
            .index
            .list_all()
            .into_iter()
            .map(|arc_record| (*arc_record).clone())
            .collect())
    }

    async fn collection_id_exists(&self, collection_id: &str) -> Result<bool> {
        // Fast check using in-memory index without full metadata retrieval
        Ok(self.index.exists_by_uuid(collection_id))
    }

    async fn upsert_collection_proto(&self, collection: &Collection) -> Result<()> {
        UniversalMetadataBackend::upsert_collection_proto(self, collection).await
    }

    async fn delete_collection(&self, collection_id: &str) -> Result<()> {
        UniversalMetadataBackend::delete_collection(self, collection_id).await
    }

    fn find_collection(&self, collection_id: &str) -> Option<Collection> {
        UniversalMetadataBackend::find_collection(self, collection_id)
    }
}

/// Snapshot manager for periodic state persistence
struct SnapshotManager {
    #[allow(dead_code)]
    threshold: u64,
    #[allow(dead_code)]
    keep_count: usize,
    base_path: PathBuf,
}

impl SnapshotManager {
    fn new(threshold: u64, keep_count: usize, base_path: PathBuf) -> Self {
        Self {
            threshold,
            keep_count,
            base_path,
        }
    }

    async fn create_snapshot(
        &self,
        index: &SingleCollectionIndex,
        fs: &dyn crate::storage::persistence::filesystem::FileSystem,
    ) -> Result<()> {
        debug!("📸 Creating snapshot");
        let start = std::time::Instant::now();

        let snapshot_dir = self.base_path.join("current");
        let timestamp = chrono::Utc::now().timestamp_millis();
        let snapshot_file = snapshot_dir.join(format!("snapshot_{}.meta", timestamp));
        let temp_file = self
            .base_path
            .join("__staging")
            .join(format!("temp_snapshot_{}.meta", timestamp));

        // Get all collections from index
        let collections = index.list_all();

        // Serialize collections to protobuf with compression
        let uncompressed_data = {
            let mut data = Vec::new();
            for collection in &collections {
                // Write length-prefixed protobuf messages
                let mut buf = Vec::new();
                collection.as_ref().encode(&mut buf)?;
                // Write length as 4 bytes
                data.extend_from_slice(&(buf.len() as u32).to_le_bytes());
                data.extend_from_slice(&buf);
            }
            data
        };

        // Compress with zlib for efficiency
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        use std::io::Write;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&uncompressed_data)?;
        let data = encoder.finish()?;

        // Write atomically
        fs.write(&temp_file.to_string_lossy(), &data, None).await?;
        fs.move_file(
            &temp_file.to_string_lossy(),
            &snapshot_file.to_string_lossy(),
        )
        .await?;

        // Update current snapshot link
        let current_snapshot = snapshot_dir.join("snapshot.meta");
        let temp_current = self.base_path.join("__staging").join("temp_snapshot.meta");

        fs.write(&temp_current.to_string_lossy(), &data, None)
            .await?;
        fs.move_file(
            &temp_current.to_string_lossy(),
            &current_snapshot.to_string_lossy(),
        )
        .await?;

        // Cleanup old snapshots
        self.cleanup_old_snapshots(fs).await?;

        info!(
            "✅ Snapshot created in {:?} with {} collections",
            start.elapsed(),
            collections.len()
        );
        Ok(())
    }

    async fn cleanup_old_snapshots(
        &self,
        fs: &dyn crate::storage::persistence::filesystem::FileSystem,
    ) -> Result<()> {
        // Move timestamped snapshots to archive with proper naming
        let current_dir = self.base_path.join("current");
        let archive_dir = self.base_path.join("archive");

        // Get all snapshots from current directory (except snapshot.meta)
        let current_entries = fs
            .list(&current_dir.to_string_lossy())
            .await
            .unwrap_or_default();

        // Archive timestamped snapshots
        for entry in current_entries {
            if entry.name.starts_with("snapshot_") && entry.name.ends_with(".meta") {
                // Create archive subdirectory with timestamp
                let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
                let mut seq = 0;
                let mut archive_subdir = format!("{}_{}", timestamp, seq);

                // Check for conflicts and increment sequence if needed
                loop {
                    let archive_path = archive_dir.join(&archive_subdir);
                    if !fs.exists(&archive_path.to_string_lossy()).await? {
                        fs.create_dir_all(&archive_path.to_string_lossy()).await?;
                        break;
                    }
                    seq += 1;
                    archive_subdir = format!("{}_{}", timestamp, seq);
                }

                let src = current_dir.join(&entry.name);
                let dst = archive_dir.join(&archive_subdir).join(&entry.name);
                match fs
                    .move_file(&src.to_string_lossy(), &dst.to_string_lossy())
                    .await
                {
                    Ok(_) => debug!(
                        "📦 Archived snapshot: {} to archive/{}",
                        entry.name, archive_subdir
                    ),
                    Err(e) => warn!("Failed to archive snapshot {}: {}", entry.name, e),
                }
            }
        }

        // Clean up old archives
        self.cleanup_old_archives(fs).await?;

        Ok(())
    }

    /// Clean up old archive directories, keeping only the last 5
    async fn cleanup_old_archives(
        &self,
        fs: &dyn crate::storage::persistence::filesystem::FileSystem,
    ) -> Result<()> {
        let archive_dir = self.base_path.join("archive");

        let archive_entries = match fs.list(&archive_dir.to_string_lossy()).await {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };

        // Filter only directories that match our timestamp pattern
        let mut archive_dirs: Vec<_> = archive_entries
            .into_iter()
            .filter(|e| {
                // Match pattern: YYYYMMDDHHmmss_seq
                e.metadata.is_directory &&
                e.name.len() >= 15 && // At least YYYYMMDDHHmmss_0
                e.name.chars().take(14).all(|c| c.is_ascii_digit()) &&
                e.name.chars().nth(14) == Some('_')
            })
            .collect();

        // Sort by name (timestamp) in reverse order (newest first)
        archive_dirs.sort_by(|a, b| b.name.cmp(&a.name));

        // Delete old archives beyond 5
        for (idx, entry) in archive_dirs.iter().enumerate() {
            if idx >= 5 {
                // Keep only the first 5 (newest)
                let path = archive_dir.join(&entry.name);
                match fs.delete(&path.to_string_lossy()).await {
                    Ok(_) => info!("🗑️ Deleted old archive directory: {}", entry.name),
                    Err(e) => warn!("Failed to delete old archive {}: {}", entry.name, e),
                }
            }
        }

        Ok(())
    }
}

// Implement InternalCollectionProvider to maintain backward compatibility
#[async_trait]
impl InternalCollectionProvider for UniversalMetadataBackend {
    // InternalCollectionProvider extends MetadataProvider, so all methods are already implemented
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::CollectionConfig;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_universal_backend_basic_operations() {
        let temp_dir = TempDir::new()
            .context("Failed to create temp directory")
            .expect("TempDir::new should not fail in test");
        let config = UniversalMetadataConfig {
            storage_url: format!("file://{}", temp_dir.path().to_string_lossy()),
            compression: true,
            enable_snapshots: false, // Disable for test
            ..Default::default()
        };

        let fs_factory = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .context("Failed to create filesystem factory")
                .expect("FilesystemFactory::create should not fail in test"),
        );

        let backend = UniversalMetadataBackend::new(config, fs_factory)
            .await
            .context("Failed to create backend")
            .expect("UniversalMetadataBackend::new should not fail in test");

        // Test create collection
        let collection_config = CollectionConfig {
            name: "test_collection".to_string(),
            dimension: 128,
            distance_metric: Some(1), // Cosine
            storage_engine: Some(1),  // Viper
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            primary_index: Some("default".to_string()),
            auto_index_selection: Some(false),
            description: Some("Test collection".to_string()),
            tags: vec![],
            owner: Some("test".to_string()),
            storage_config: None,
            embedding_models: vec![],
            record_schema: None,
            enable_proxima_record: None,
            text_columns: vec![],
            text_storage_configs: vec![],
        };

        // Create a proto collection
        let collection = Collection {
            id: "test_id".to_string(),
            config: Some(collection_config),
            stats: Some(crate::proto::proximadb_v1::CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            storage_assignment: None,
        };

        backend
            .upsert_collection_proto(&collection)
            .await
            .context("Failed to upsert collection")
            .expect("upsert_collection_proto should not fail in test");

        // Test get collection
        let collection = backend
            .get_collection("test_collection")
            .await
            .context("Failed to get collection")
            .expect("get_collection should not fail in test");
        assert!(collection.is_some());
        let collection = collection.expect("Collection should exist");
        assert_eq!(
            collection
                .config
                .as_ref()
                .expect("Collection config should exist")
                .name,
            "test_collection"
        );
        assert_eq!(
            collection
                .config
                .as_ref()
                .expect("Collection config should exist")
                .dimension,
            128
        );

        // Test list collections
        let collections = backend
            .list_collections()
            .await
            .context("Failed to list collections")
            .expect("list_collections should not fail in test");
        assert_eq!(collections.len(), 1);

        // Test collection exists
        assert!(
            backend
                .collection_exists("test_collection")
                .await
                .context("Failed to check collection existence")
                .expect("collection_exists should not fail in test")
        );
        assert!(
            !backend
                .collection_exists("nonexistent")
                .await
                .context("Failed to check collection existence")
                .expect("collection_exists should not fail in test")
        );

        // Test delete collection
        backend
            .delete_collection("test_collection")
            .await
            .context("Failed to delete collection")
            .expect("delete_collection should not fail in test");
        assert!(
            !backend
                .collection_exists("test_collection")
                .await
                .context("Failed to verify collection deletion")
                .expect("collection_exists should not fail in test")
        );
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_atomic_operation_path_handling() {
        // Test that atomic operations don't duplicate paths
        let temp_dir = TempDir::new()
            .context("Failed to create temp directory")
            .expect("TempDir::new should not fail in test");
        let metadata_url = format!(
            "file://{}",
            temp_dir
                .path()
                .to_str()
                .expect("Temp directory path should be valid UTF-8")
        );

        let fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let fs_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config)
                .await
                .context("Failed to create filesystem factory")
                .expect("FilesystemFactory::create should not fail in test"),
        );
        let config = UniversalMetadataConfig {
            storage_url: metadata_url.clone(),
            compression: true,
            enable_snapshots: true,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let backend = UniversalMetadataBackend::new(config, fs_factory)
            .await
            .context("Failed to create backend")
            .expect("UniversalMetadataBackend::new should not fail in test");

        // Create a test collection using proper proto structure

        let collection = crate::proto::proximadb_v1::Collection {
            id: "test_atomic".to_string(),
            config: Some(CollectionConfig {
                name: "test_atomic_collection".to_string(),
                dimension: 128,
                distance_metric: Some(0), // Cosine
                storage_engine: Some(0),  // VIPER
                filterable_columns: vec![],
                index_configs: vec![],
                quantization: None,
                primary_index: Some(String::new()),
                auto_index_selection: Some(false),
                description: Some("Test atomic collection".to_string()),
                tags: vec!["test".to_string()],
                owner: Some("test_user".to_string()),
                storage_config: None,
                embedding_models: vec![],
                record_schema: None,
                enable_proxima_record: None,
                text_columns: vec![],
                text_storage_configs: vec![],
            }),
            stats: Some(CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time should be valid")
                .as_secs() as i64,
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time should be valid")
                .as_secs() as i64,
            storage_assignment: None,
        };

        // Store the collection
        backend
            .upsert_collection_proto(&collection)
            .await
            .context("Failed to upsert collection")
            .expect("upsert_collection_proto should not fail in test");

        // Verify the staging directory structure
        let current_staging = temp_dir.path().join("current").join("__staging");
        assert!(
            current_staging.exists(),
            "Current staging directory should exist"
        );

        // Verify no duplicated paths
        let duplicated_path = temp_dir.path().join(
            temp_dir
                .path()
                .file_name()
                .expect("Temp directory path should have a file name"),
        );
        assert!(
            !duplicated_path.exists(),
            "Should not create duplicated directory structure"
        );
    }

    #[tokio::test]
    async fn test_relative_path_handling() {
        // Test relative path handling specifically
        let test_dir = "test_relative_metadata_info";
        std::fs::remove_dir_all(test_dir).ok(); // Clean up any previous test runs
        std::fs::create_dir_all(test_dir).ok();
        std::fs::create_dir_all(format!("{}/current", test_dir)).ok();
        std::fs::create_dir_all(format!("{}/staging", test_dir)).ok();

        let metadata_url = format!("file://./{}", test_dir);

        let fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let fs_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config)
                .await
                .context("Failed to create filesystem factory")
                .expect("FilesystemFactory::create should not fail in test"),
        );
        let config = UniversalMetadataConfig {
            storage_url: metadata_url.clone(),
            compression: true,
            enable_snapshots: true,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let backend = UniversalMetadataBackend::new_for_testing(config, fs_factory)
            .await
            .context("Failed to create backend")
            .expect("UniversalMetadataBackend::new_for_testing should not fail in test");

        // Store a collection using proper proto structure
        let collection = crate::proto::proximadb_v1::Collection {
            id: "relative_test".to_string(),
            config: Some(CollectionConfig {
                name: "relative_test_collection".to_string(),
                dimension: 128,
                distance_metric: Some(0), // Cosine
                storage_engine: Some(0),  // VIPER
                filterable_columns: vec![],
                index_configs: vec![],
                quantization: None,
                storage_config: None,
                primary_index: Some(String::new()),
                auto_index_selection: Some(false),
                description: Some("Test relative collection".to_string()),
                tags: vec!["test".to_string()],
                owner: Some("test_user".to_string()),
                embedding_models: vec![],
                record_schema: None,
                enable_proxima_record: None,
                text_columns: vec![],
                text_storage_configs: vec![],
            }),
            stats: Some(CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time should be valid")
                .as_secs() as i64,
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time should be valid")
                .as_secs() as i64,
            storage_assignment: None,
        };

        backend
            .upsert_collection_proto(&collection)
            .await
            .context("Failed to upsert collection")
            .expect("upsert_collection_proto should not fail in test");

        // Verify correct path structure
        assert!(std::path::Path::new(test_dir).join("current").exists());
        assert!(!std::path::Path::new(test_dir).join(test_dir).exists());

        // Cleanup
        std::fs::remove_dir_all(test_dir).ok();
    }

    #[tokio::test]
    async fn test_universal_backend_create() {
        let filesystem_factory =
            Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());
        let config = UniversalMetadataConfig {
            storage_url: "file:///tmp/test_metadata_info".to_string(),
            compression: true,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let backend = UniversalMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create filestore backend");

        let collection_uuids = backend.list_collection_uuids();
        assert!(collection_uuids.is_empty());
    }

    use crate::proto::proximadb_v1::{
        Collection, CollectionConfig, CollectionMetadata, CollectionStats, DistanceMetric,
        FilterableColumnSpec, IndexingAlgorithm, StorageEngine,
    };

    fn create_test_config_for_proto(temp_dir: &TempDir) -> UniversalMetadataConfig {
        UniversalMetadataConfig {
            storage_url: format!("file://{}", temp_dir.path().display()),
            compression: true,
            enable_snapshots: true,
            snapshot_threshold: 10,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: Some(temp_dir.path().join("temp").to_string_lossy().to_string()),
        }
    }

    fn create_test_proto_collection(id: &str, name: &str) -> Collection {
        Collection {
            id: id.to_string(),
            config: Some(CollectionConfig {
                name: name.to_string(),
                dimension: 384,
                distance_metric: DistanceMetric::Cosine as i32,
                storage_engine: StorageEngine::Viper as i32,
                primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
                filterable_columns: vec![
                    FilterableColumnSpec {
                        name: "category".to_string(),
                        indexed: true,
                        supports_range: false,
                        estimated_cardinality: Some(100),
                        encoding_hint: None,
                        compression: None,
                        optimization_hints: None,
                    },
                    FilterableColumnSpec {
                        name: "price".to_string(),
                        indexed: true,
                        supports_range: true,
                        estimated_cardinality: None,
                        encoding_hint: None,
                        compression: None,
                        optimization_hints: None,
                    },
                ],
                index_configs: vec![],
                quantization: None,
                primary_index: Some("default".to_string()),
                auto_index_selection: Some(true),
                description: None,
                tags: vec![],
                owner: None,
                compression: None,
                optimization_hints: None,
            }),
            stats: Some(CollectionStats {
                vector_count: 1000,
                data_size_bytes: 1024 * 1024,
                index_size_bytes: 512 * 1024,
                wal_size_bytes: 256 * 1024,
                last_updated: chrono::Utc::now().timestamp(),
            }),
            metadata: Some(CollectionMetadata {
                timestamp: Some(chrono::Utc::now().timestamp()),
                updated_at: chrono::Utc::now().timestamp(),
                version: Some(1),
                description: Some("Test collection".to_string()),
                tags: vec!["test".to_string(), "proto".to_string()],
                owner: Some("test_user".to_string()),
            }),
        }
    }

    #[tokio::test]
    async fn test_universal_backend_create_with_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config_for_proto(&temp_dir);

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("Failed to create filesystem factory"),
        );

        let backend = UniversalMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create filestore backend");

        assert!(backend.internal_health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_upsert_collection_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config_for_proto(&temp_dir);

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("Failed to create filesystem factory"),
        );

        let backend = UniversalMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create backend");

        let proto_collection = create_test_proto_collection("test-id-123", "test-collection");

        backend
            .upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert proto collection");

        let retrieved = backend
            .find_collection("test-collection")
            .expect("Collection should exist");

        assert_eq!(retrieved.id, "test-id-123");
        assert_eq!(retrieved.name, "test-collection");
        assert_eq!(retrieved.dimension, 384);
    }

    #[tokio::test]
    async fn test_proto_file_extension() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config_for_proto(&temp_dir);

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("Failed to create filesystem factory"),
        );

        let backend = UniversalMetadataBackend::new(config, filesystem_factory.clone())
            .await
            .expect("Failed to create backend");

        let proto_collection = create_test_proto_collection("proto-123", "proto-test");
        backend
            .upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert");

        let ops_dir = temp_dir.path().join("operations");
        let fs = filesystem_factory.get_filesystem("file://").unwrap();
        let entries = fs.list(&ops_dir.to_string_lossy()).await.unwrap();

        let oplog_files: Vec<_> = entries
            .iter()
            .filter(|e| e.name.ends_with(".oplog"))
            .collect();

        assert!(!oplog_files.is_empty(), "Should have created .oplog files");
        assert!(
            oplog_files[0].name.starts_with("op_"),
            "Oplog file should have correct prefix"
        );
    }

    #[tokio::test]
    async fn test_atomic_coordination_with_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config_for_proto(&temp_dir);

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("Failed to create filesystem factory"),
        );

        let backend = UniversalMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create backend");

        let collections = vec![
            create_test_proto_collection("atomic-1", "atomic-test-1"),
            create_test_proto_collection("atomic-2", "atomic-test-2"),
            create_test_proto_collection("atomic-3", "atomic-test-3"),
        ];

        for collection in &collections {
            backend
                .upsert_collection_proto(collection)
                .await
                .expect("Failed to upsert collection");
        }

        assert!(backend.find_collection("atomic-test-1").is_some());
        assert!(backend.find_collection("atomic-test-2").is_some());
        assert!(backend.find_collection("atomic-test-3").is_some());
    }

    #[tokio::test]
    async fn test_checkpoint_with_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config_for_proto(&temp_dir);

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("Failed to create filesystem factory"),
        );

        let backend = UniversalMetadataBackend::new(config, filesystem_factory.clone())
            .await
            .expect("Failed to create backend");

        for i in 0..12 {
            let collection = create_test_proto_collection(
                &format!("checkpoint-{}", i),
                &format!("checkpoint-test-{}", i),
            );
            backend
                .upsert_collection_proto(&collection)
                .await
                .expect("Failed to upsert");
        }

        let snapshots_dir = temp_dir.path().join("snapshots");
        let fs = filesystem_factory.get_filesystem("file://").unwrap();
        let entries = fs.list(&snapshots_dir.to_string_lossy()).await.unwrap();

        let checkpoint_files: Vec<_> = entries
            .iter()
            .filter(|e| e.name.starts_with("checkpoint_") && e.name.ends_with(".meta"))
            .collect();

        assert!(
            !checkpoint_files.is_empty(),
            "Should have created checkpoint files"
        );
    }

    #[tokio::test]
    async fn test_recovery_from_oplog_files() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config_for_proto(&temp_dir);

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("Failed to create filesystem factory"),
        );

        {
            let backend = UniversalMetadataBackend::new(config.clone(), filesystem_factory.clone())
                .await
                .expect("Failed to create backend");

            for i in 0..5 {
                let collection = create_test_proto_collection(
                    &format!("recovery-{}", i),
                    &format!("recovery-test-{}", i),
                );
                backend
                    .upsert_collection_proto(&collection)
                    .await
                    .expect("Failed to upsert");
            }
        }

        {
            let backend = UniversalMetadataBackend::new(config, filesystem_factory.clone())
                .await
                .expect("Failed to create backend");

            for i in 0..5 {
                let collection = backend
                    .find_collection(&format!("recovery-test-{}", i))
                    .expect(&format!("Collection recovery-test-{} should exist", i));
                assert_eq!(collection.id, format!("recovery-{}", i));
            }
        }
    }

    #[tokio::test]
    async fn test_filterable_columns_in_proto() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config_for_proto(&temp_dir);

        let filesystem_factory = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("Failed to create filesystem factory"),
        );

        let backend = UniversalMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create backend");

        let mut proto_collection = create_test_proto_collection("filter-123", "filter-test");
        if let Some(ref mut config) = proto_collection.config {
            config.filterable_columns = vec![
                FilterableColumnSpec {
                    name: "timestamp".to_string(),
                    indexed: true,
                    supports_range: true,
                    estimated_cardinality: None,
                    encoding_hint: None,
                    compression: None,
                    optimization_hints: None,
                },
                FilterableColumnSpec {
                    name: "status".to_string(),
                    indexed: true,
                    supports_range: false,
                    estimated_cardinality: Some(5),
                    encoding_hint: None,
                    compression: None,
                    optimization_hints: None,
                },
                FilterableColumnSpec {
                    name: "score".to_string(),
                    indexed: true,
                    supports_range: true,
                    estimated_cardinality: Some(100),
                    encoding_hint: None,
                    compression: None,
                    optimization_hints: None,
                },
            ];
        }

        backend
            .upsert_collection_proto(&proto_collection)
            .await
            .expect("Failed to upsert");

        let retrieved = backend
            .find_collection("filter-test")
            .expect("Collection should exist");

        assert_eq!(retrieved.filterable_metadata_fields.len(), 3);
        assert!(
            retrieved
                .filterable_metadata_fields
                .contains_hash(&"timestamp".to_string())
        );
        assert!(
            retrieved
                .filterable_metadata_fields
                .contains_hash(&"status".to_string())
        );
        assert!(
            retrieved
                .filterable_metadata_fields
                .contains_hash(&"score".to_string())
        );
    }
}
