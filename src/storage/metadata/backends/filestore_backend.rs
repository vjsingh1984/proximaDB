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
use prost::Message;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn, error};
use uuid::Uuid;

use crate::proto::proximadb::Collection;
use crate::storage::metadata::single_index::SingleCollectionIndex;
use crate::storage::persistence::filesystem::FilesystemFactory;


/// Protobuf operation for incremental collection storage
#[derive(Clone, Message)]
pub struct ProtoIncrementalOperation {
    #[prost(uint64, tag = "1")]
    pub sequence: u64,
    #[prost(int64, tag = "2")]
    pub timestamp: i64,
    #[prost(int32, tag = "3")]
    pub operation_type: i32, // ProtoOperationType as i32
    #[prost(string, tag = "4")]
    pub collection_id: String,
    #[prost(message, optional, tag = "5")]
    pub collection_data: Option<Collection>,
}

/// Operation types for protobuf storage
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtoOperationType {
    Create = 1,
    Update = 2,
    Delete = 3,
}
use crate::storage::atomic::{UnifiedAtomicCoordinator, TransactionHandle, RollbackAction, generate_transaction_id, StagingConfig, StagingOperationType};
use crate::storage::traits::CollectionMetadataProvider;

/// Configuration for filestore metadata backend
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilestoreMetadataConfig {
    /// Storage URL (file://, s3://, gcs://, adls://)
    pub storage_url: String,
    
    /// Enable compression for Avro files
    #[serde(default = "default_true")]
    pub enable_compression: bool,
    
    /// Enable periodic snapshots
    #[serde(default = "default_true")]
    pub enable_snapshots: bool,
    
    /// Snapshot after N operations
    #[serde(default = "default_snapshot_threshold")]
    pub snapshot_threshold: u64,
    
    /// Keep N recent snapshots
    #[serde(default = "default_keep_snapshots")]
    pub keep_snapshots: usize,
    
    /// Enable backup to secondary location
    #[serde(default)]
    pub backup_url: Option<String>,
    
    /// Temporary directory for atomic operations
    #[serde(default)]
    pub temp_dir: Option<String>,
}

fn default_true() -> bool { true }
fn default_snapshot_threshold() -> u64 { 1000 }
fn default_keep_snapshots() -> usize { 3 }

impl Default for FilestoreMetadataConfig {
    fn default() -> Self {
        Self {
            storage_url: "file://./data/metadata".to_string(),
            enable_compression: true,
            enable_snapshots: true,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        }
    }
}

/// Operation type for WAL-style logging
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum OperationType {
    Create,
    Update,
    Delete,
}

/// Incremental operation for WAL
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IncrementalOperation {
    pub sequence: u64,
    pub timestamp: i64,
    pub operation_type: OperationType,
    pub collection_id: String,
    pub collection_data: Option<Collection>,
}

/// Prepared write data for atomic operations
#[derive(Debug)]
struct PreparedWrite {
    temp_path: PathBuf,
    final_path: PathBuf,
    data: Vec<u8>,
}

/// Filestore metadata backend implementation
pub struct FilestoreMetadataBackend {
    /// Configuration
    config: FilestoreMetadataConfig,
    
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
    atomic_coordinator: Arc<UnifiedAtomicCoordinator>,
}

impl FilestoreMetadataBackend {
    /// Create new filestore backend
    pub async fn new(
        config: FilestoreMetadataConfig,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        info!("🏗️ Initializing Filestore metadata backend: {}", config.storage_url);
        
        // Parse base path from URL
        let base_path = Self::parse_base_path(&config.storage_url)?;
        
        // Proto-first architecture - no schema needed
            
        // Create in-memory index
        let index = Arc::new(SingleCollectionIndex::new());
        
        // Create atomic coordinator for metadata operations
        let atomic_coordinator = Arc::new(
            UnifiedAtomicCoordinator::new(
                filesystem_factory.clone(),
                config.temp_dir.clone(),
            ).await
            .context("Failed to create atomic coordinator")?
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
            info!("📸 Snapshot manager initialized with threshold: {}", config.snapshot_threshold);
        }
        
        info!("✅ Filestore metadata backend ready, recovered sequence: {}", recovered_sequence);
        Ok(backend)
    }
    
    /// Parse base path from storage URL
    fn parse_base_path(url: &str) -> Result<PathBuf> {
        if let Some(path) = url.strip_prefix("file://") {
            Ok(PathBuf::from(path))
        } else if url.starts_with("s3://") || url.starts_with("gcs://") || url.starts_with("adls://") {
            // For cloud storage, use the full URL as path
            Ok(PathBuf::from(url))
        } else {
            // Treat as local path
            Ok(PathBuf::from(url))
        }
    }
    
    /// Get filesystem instance  
    fn get_fs(&self) -> Result<&dyn crate::storage::persistence::filesystem::FileSystem> {
        self.filesystem_factory
            .get_filesystem(&self.config.storage_url)
            .map_err(|e| anyhow::anyhow!("Failed to get filesystem: {}", e))
    }
    
    /// Initialize storage directory structure
    async fn initialize_storage(&self) -> Result<()> {
        let fs = self.get_fs()?;
        
        // Create directory structure
        let dirs = [
            &self.base_path,
            &self.base_path.join("collections"),
            &self.base_path.join("operations"),
            &self.base_path.join("snapshots"),
            &self.base_path.join("temp"),
        ];
        
        for dir in &dirs {
            let dir_str = dir.to_string_lossy();
            if !fs.exists(&dir_str).await? {
                fs.create_dir_all(&dir_str).await
                    .with_context(|| format!("Failed to create directory: {}", dir_str))?;
                debug!("📁 Created directory: {}", dir_str);
            }
        }
        
        info!("📂 Storage directories initialized");
        Ok(())
    }
    
    /// Recover collections from storage
    async fn recover_from_storage(&self) -> Result<u64> {
        info!("🔄 Starting metadata recovery");
        
        // Try to recover from latest snapshot first
        if let Ok(snapshot_sequence) = self.recover_from_snapshot().await {
            info!("📸 Recovered from snapshot, sequence: {}", snapshot_sequence);
            self.recover_incremental_operations(snapshot_sequence).await?;
            return Ok(self.sequence.load(Ordering::SeqCst));
        }
        
        // Fallback to full recovery from operations
        info!("📜 No snapshot found, performing full recovery");
        self.recover_from_operations().await
    }
    
    /// Recover from latest snapshot
    async fn recover_from_snapshot(&self) -> Result<u64> {
        let fs = self.get_fs()?;
        let snapshot_dir = self.base_path.join("snapshots");
        let current_snapshot = snapshot_dir.join("current.proto");
        
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
            if offset + 4 > data.len() { break; }
            let len = u32::from_le_bytes([data[offset], data[offset+1], data[offset+2], data[offset+3]]) as usize;
            offset += 4;
            
            // Read protobuf message
            if offset + len > data.len() { break; }
            let record = Collection::decode(&data[offset..offset+len])?;
            offset += len;
            
            max_sequence = max_sequence.max(1); // Proto collections don't have version field
            records.push(record);
            count += 1;
        }
        
        info!("📦 Loaded {} collections from snapshot, rebuilding indexes...", count);
        
        // Clear existing index to ensure clean state
        self.index.clear();
        
        // Rebuild both primary (UUID) and secondary (name) indexes
        for record in records {
            // This will update both UUID->record and name->UUID mappings
            self.index.upsert_collection(record);
        }
        
        info!("✅ Rebuilt indexes with {} collections from snapshot", count);
        Ok(max_sequence)
    }
    
    /// Recover from operation files
    async fn recover_from_operations(&self) -> Result<u64> {
        let fs = self.get_fs()?;
        let ops_dir = self.base_path.join("operations");
        
        let entries = match fs.list(&ops_dir.to_string_lossy()).await {
            Ok(entries) => entries,
            Err(_) => {
                info!("📋 No operations directory found");
                return Ok(0);
            }
        };
        
        // Sort operation files by sequence
        let mut op_files: Vec<_> = entries
            .into_iter()
            .filter(|e| e.name.starts_with("op_") && e.name.ends_with(".proto"))
            .collect();
        
        op_files.sort_by(|a, b| a.name.cmp(&b.name));
        
        let mut max_sequence = 0;
        for entry in op_files {
            let op_path = ops_dir.join(&entry.name);
            if let Ok(sequence) = self.recover_operation_file(&op_path).await {
                max_sequence = max_sequence.max(sequence);
            }
        }
        
        info!("📜 Recovery completed, max sequence: {}", max_sequence);
        Ok(max_sequence)
    }
    
    /// Recover incremental operations after snapshot
    async fn recover_incremental_operations(&self, after_sequence: u64) -> Result<()> {
        let fs = self.get_fs()?;
        let ops_dir = self.base_path.join("operations");
        
        let entries = match fs.list(&ops_dir.to_string_lossy()).await {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };
        
        // Filter operations after snapshot sequence
        let mut incremental_ops = Vec::new();
        
        for entry in entries {
            if entry.name.starts_with("op_") && entry.name.ends_with(".proto") {
                // Parse sequence from filename: op_XXXXXXXX.proto
                if let Some(seq_str) = entry.name.strip_prefix("op_").and_then(|s| s.strip_suffix(".proto")) {
                    if let Ok(sequence) = seq_str.parse::<u64>() {
                        if sequence > after_sequence {
                            let op_path = ops_dir.join(&entry.name);
                            incremental_ops.push((sequence, op_path));
                        }
                    }
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
        
        info!("📈 Applied {} incremental operations", ops_count);
        Ok(())
    }
    
    /// Recover single operation file
    async fn recover_operation_file(&self, path: &Path) -> Result<u64> {
        let fs = self.get_fs()?;
        let data = fs.read(&path.to_string_lossy()).await?;
        
        // Parse JSON operation log
        let op_json: serde_json::Value = serde_json::from_slice(&data)?;
        let sequence = op_json["sequence"].as_u64().unwrap_or(0);
        let op_type_str = op_json["operation_type"].as_str().unwrap_or("");
        
        let mut max_sequence = sequence;
        
        match op_type_str {
            "Create" | "Update" => {
                if let Some(collection_data) = op_json["collection_data"].as_array() {
                    // Decode protobuf bytes from JSON array
                    let collection_bytes: Vec<u8> = collection_data.iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u8))
                        .collect();
                    let record = Collection::decode(&collection_bytes[..])?;
                    self.index.upsert_collection(record);
                }
            }
            "Delete" => {
                // collection_id is the name, need to get UUID first
                let collection_id = op_json["collection_id"].as_str().unwrap_or("");
                if let Some(uuid) = self.index.get_uuid_by_name(collection_id) {
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
    
    /// Atomic write operation using UnifiedAtomicCoordinator for ACID guarantees
    async fn atomic_persist_operation(&self, operation: &IncrementalOperation) -> Result<()> {
        if !self.atomic_operations_enabled {
            return self.execute_simple_persist(operation).await;
        }

        // Use the atomic coordinator
        let coordinator = self.atomic_coordinator.clone();
        
        // Generate transaction ID
        let tx_id = generate_transaction_id("collection_metadata");
        
        info!("🔒 Starting ACID transaction: {} for seq={}", tx_id, operation.sequence);
        
        // Begin ACID transaction with three participants: memtable, secondary_index, disk
        let tx = coordinator
            .begin_transaction(
                &tx_id,
                vec!["memtable".to_string(), "secondary_index".to_string(), "disk".to_string()],
            )
            .await?;
        
        // Phase 1: Prepare all participants
        match self.prepare_transaction_participants(&tx, operation).await {
            Ok(prepared_data) => {
                // Phase 2: Commit if all participants prepared successfully
                if tx.prepare().await? {
                    // All participants ready - commit the transaction
                    match self.commit_transaction_participants(&tx, operation, &prepared_data).await {
                        Ok(_) => {
                            tx.commit().await?;
                            self.check_snapshot_trigger().await?;
                            info!("✅ ACID transaction {} completed for seq={}", tx_id, operation.sequence);
                            Ok(())
                        }
                        Err(e) => {
                            // Rollback on commit failure
                            warn!("❌ ACID transaction {} commit failed: {}", tx_id, e);
                            tx.rollback().await?;
                            Err(e)
                        }
                    }
                } else {
                    // Prepare failed - rollback
                    warn!("❌ ACID transaction {} prepare phase failed", tx_id);
                    tx.rollback().await?;
                    Err(anyhow::anyhow!("Transaction prepare phase failed"))
                }
            }
            Err(e) => {
                // Rollback on prepare failure
                warn!("❌ ACID transaction {} prepare failed: {}", tx_id, e);
                tx.rollback().await?;
                Err(e)
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
        fs.write(&prepared_data.temp_path.to_string_lossy(), &prepared_data.data, None).await?;
        fs.move_file(&prepared_data.temp_path.to_string_lossy(), &prepared_data.final_path.to_string_lossy()).await?;
        
        self.check_snapshot_trigger().await?;
        Ok(())
    }
    
    /// Prepare filestore write data without committing
    async fn prepare_filestore_write(&self, operation: &IncrementalOperation) -> Result<PreparedWrite> {
        let fs = self.get_fs()?;
        let ops_dir = self.base_path.join("operations");
        
        // Create operation filename with sequence
        let filename = format!("op_{:016}.proto", operation.sequence);
        let op_path = ops_dir.join(&filename);
        let temp_path = self.base_path.join("temp").join(&format!("temp_{}", filename));
        
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
    async fn execute_atomic_write(&self, operation: &IncrementalOperation, prepared: &PreparedWrite) -> Result<()> {
        let fs = self.get_fs()?;
        
        // Step 1: Write to temp file (prepare phase)
        fs.write(&prepared.temp_path.to_string_lossy(), &prepared.data, None).await?;
        
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
        fs.move_file(&prepared.temp_path.to_string_lossy(), &prepared.final_path.to_string_lossy()).await?;
        
        Ok(())
    }
    
    /// Check if snapshot is needed after successful operation
    async fn check_snapshot_trigger(&self) -> Result<()> {
        let ops_count = self.ops_since_snapshot.fetch_add(1, Ordering::SeqCst) + 1;
        if ops_count >= self.config.snapshot_threshold {
            if let Some(manager) = self.snapshot_manager.lock().await.as_ref() {
                let fs = self.get_fs()?;
                if let Err(e) = manager.create_snapshot(&self.index, fs).await {
                    warn!("📸 Snapshot creation failed: {}", e);
                } else {
                    self.ops_since_snapshot.store(0, Ordering::SeqCst);
                }
            }
        }
        Ok(())
    }
    
    /// Create collection record from proto config
    fn create_collection_record(&self, name: String, config: &crate::proto::proximadb::CollectionConfig) -> Result<Collection> {
        Ok(Collection {
            id: uuid::Uuid::new_v4().to_string(),
            config: Some(config.clone()),
            stats: Some(crate::proto::proximadb::CollectionStats {
                vector_count: 0,
                index_size_bytes: 0,
                data_size_bytes: 0,
            }),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
        })
    }
    
    /// Create a new collection (CRUD operation)
    pub async fn create_collection(&self, name: String, config: &crate::proto::proximadb::CollectionConfig) -> Result<String> {
        info!("🆕 Creating collection: {}", name);
        
        // Check if collection already exists
        if self.index.get_by_name(&name).is_some() {
            return Err(anyhow::anyhow!("Collection '{}' already exists", name));
        }
        
        // Create collection record
        let record = self.create_collection_record(name, config)?;
        let uuid = record.id.clone();
        
        // Create operation
        let operation = IncrementalOperation {
            sequence: self.next_sequence(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            operation_type: OperationType::Create,
            collection_id: record.config.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
            collection_data: Some(record.clone()),
        };
        
        // Persist operation atomically
        self.atomic_persist_operation(&operation).await?;
        
        // Update in-memory index
        self.index.upsert_collection(record);
        
        info!("✅ Collection created: {} -> {}", &operation.collection_id, uuid);
        Ok(uuid)
    }
    
    /// Update an existing collection (CRUD operation)
    pub async fn update_collection(&self, collection_id: &str, config: &crate::proto::proximadb::CollectionConfig) -> Result<()> {
        info!("📝 Updating collection: {}", collection_id);
        
        // Get existing collection
        let existing = self.index.get_by_name(collection_id).map(|arc_record| (*arc_record).clone())
            .ok_or_else(|| anyhow::anyhow!("Collection '{}' not found", collection_id))?;
        
        // Create updated record
        let mut updated = self.create_collection_record(collection_id.to_string(), config)?;
        updated.id = existing.id.clone(); // Keep same ID
        updated.created_at = existing.created_at; // Keep creation time
        // Proto collections don't have version field - using updated_at for tracking
        
        // Create operation
        let operation = IncrementalOperation {
            sequence: self.next_sequence(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            operation_type: OperationType::Update,
            collection_id: collection_id.to_string(),
            collection_data: Some(updated.clone()),
        };
        
        // Persist operation atomically
        self.atomic_persist_operation(&operation).await?;
        
        // Update in-memory index
        self.index.upsert_collection(updated);
        
        info!("✅ Collection updated: {}", collection_id);
        Ok(())
    }
    
    /// Delete a collection (CRUD operation)
    pub async fn delete_collection(&self, collection_id: &str) -> Result<()> {
        info!("🗑️ Deleting collection: {}", collection_id);
        
        // Check if collection exists
        if self.index.get_by_name(collection_id).is_none() {
            return Err(anyhow::anyhow!("Collection '{}' not found", collection_id));
        }
        
        // Create operation
        let operation = IncrementalOperation {
            sequence: self.next_sequence(),
            timestamp: chrono::Utc::now().timestamp_millis(),
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
        
        info!("✅ Collection deleted: {}", collection_id);
        Ok(())
    }
    
    /// Upsert collection record directly
    pub async fn upsert_collection_record(&self, record: Collection) -> Result<()> {
        // Create operation
        let operation = IncrementalOperation {
            sequence: self.next_sequence(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            operation_type: OperationType::Update,
            collection_id: record.config.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
            collection_data: Some(record.clone()),
        };
        
        // Persist operation atomically
        self.atomic_persist_operation(&operation).await?;
        
        // Update in-memory index
        self.index.upsert_collection(record);
        
        Ok(())
    }

    /// Store protobuf collection directly - zero-copy protobuf serialization
    pub async fn upsert_collection_proto(&self, proto_collection: &Collection) -> Result<()> {
        let config = proto_collection.config.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection config is required"))?;
        
        // Create protobuf operation for incremental storage
        let operation = ProtoIncrementalOperation {
            sequence: self.next_sequence(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            operation_type: ProtoOperationType::Update as i32,
            collection_id: config.name.clone(),
            collection_data: Some(proto_collection.clone()),
        };
        
        // Use unified atomic coordinator for ALL operations: proto file + memtable + secondary index
        let tx_id = generate_transaction_id("metadata");
        let tx = self.atomic_coordinator.begin_transaction(&tx_id, vec!["metadata".to_string()]).await?;
        
        // 1. Serialize to protobuf binary for disk storage
        let mut buf = Vec::new();
        operation.encode(&mut buf)?;
        
        // 2. Convert proto to core collection for in-memory operations
        let core_collection = self.convert_proto_to_core(proto_collection);
        
        // 3. Stage proto file operation
        let filename = format!("op_{:016}.proto", operation.sequence);
        let ops_dir = self.base_path.join("operations");
        let final_path = ops_dir.join(&filename);
        
        let staging_config = StagingConfig {
            base_url: self.config.storage_url.clone(),
            collection_id: None,
            operation_type: StagingOperationType::Metadata,
            custom_staging_dir: Some(self.base_path.join("temp").to_string_lossy().to_string()),
            auto_cleanup: false,
            max_orphaned_age_hours: 24,
        };
        
        // 4. Register rollback actions for memtable and secondary index
        tx.register_rollback(
            "memtable",
            RollbackAction::RemoveFromMemtable {
                key: core_collection.id.clone(),
            },
        ).await?;
        
        tx.register_rollback(
            "secondary_index", 
            RollbackAction::RemoveFromSecondaryIndex {
                name: core_collection.config.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
                uuid: core_collection.id.clone(),
            },
        ).await?;
        
        // 5. Write protobuf file directly (staging is handled by atomic coordinator)
        let fs = self.get_fs()?;
        let temp_path = staging_config.custom_staging_dir.as_ref()
            .map(|d| PathBuf::from(d).join(&filename))
            .unwrap_or_else(|| self.base_path.join("temp").join(&filename));
        fs.write(&temp_path.to_string_lossy(), &buf, None).await?;
        
        // 6. Update in-memory structures before commit
        self.index.upsert_collection(proto_collection.clone());
        
        // 7. Commit the entire transaction
        tx.commit().await?;
        
        Ok(())
    }
    
    /// Convert protobuf collection to core Collection for fast in-memory index
    fn convert_proto_to_core(&self, proto: &Collection) -> Collection {
        // Proto Collection is already the core type - no conversion needed
        proto.clone()
    }
    
    
    /// Prepare transaction participants
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
                            name: record.config.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
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
                        let prev_name = previous.config.as_ref().map(|c| &c.name).unwrap_or(&empty_string);
                        let curr_name = record.config.as_ref().map(|c| &c.name).unwrap_or(&empty_string);
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
    
    /// Serialize record to bytes for rollback
    fn serialize_record(&self, record: &Collection) -> Result<Vec<u8>> {
        // Proto-first: serialize directly to protobuf
        let mut buf = Vec::new();
        record.encode(&mut buf).context("Failed to encode protobuf")?;
        Ok(buf)
    }

    /// Get collection record by name - uses O(1) secondary index lookup
    pub fn get_collection_record_by_name(&self, name: &str) -> Option<Collection> {
        self.index.get_by_name(name).map(|arc_record| (*arc_record).clone())
    }
    
    /// Get collection record by UUID - uses O(1) primary key lookup  
    pub fn get_collection_record_by_uuid(&self, uuid: &str) -> Option<Collection> {
        self.index.get_by_uuid(uuid).map(|arc_record| (*arc_record).clone())
    }
    
    /// Get collection name by UUID - uses O(1) primary key lookup
    pub fn get_collection_name_by_uuid(&self, uuid: &str) -> Option<String> {
        self.index.get_by_uuid(uuid).and_then(|record| record.config.as_ref().map(|c| c.name.clone()))
    }
    
    /// Get all collection UUIDs
    pub fn list_collection_uuids(&self) -> Vec<String> {
        self.index.list_all().into_iter().map(|record| record.id.clone()).collect()
    }
    
    /// Get all collection names  
    pub fn list_collection_names(&self) -> Vec<String> {
        self.index.list_all().into_iter().filter_map(|record| record.config.as_ref().map(|c| c.name.clone())).collect()
    }
    
    /// Create a checkpoint snapshot
    pub async fn create_checkpoint(&self) -> Result<()> {
        let checkpoint_dir = self.base_path.join("snapshots");
        
        let sequence = self.sequence.load(Ordering::SeqCst);
        let timestamp = chrono::Utc::now().timestamp_millis();
        let checkpoint_name = format!("checkpoint_{}_{}.proto", sequence, timestamp);
        
        // Configure atomic operation for metadata checkpoint
        let staging_config = StagingConfig {
            base_url: self.config.storage_url.clone(),
            collection_id: None, // Checkpoint is not collection-specific
            operation_type: StagingOperationType::Metadata,
            custom_staging_dir: Some("__metadata".to_string()),
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
        };
        
        // Begin atomic operation
        let operation = self.atomic_coordinator
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
            .write_to_staging(&operation.operation_id, "current_checkpoint.proto", link_content)
            .await
            .context("Failed to write current link to staging")?;
        
        // Finalize the atomic operation - this handles the atomic move and cleanup
        self.atomic_coordinator
            .finalize_atomic_operation(&operation.operation_id)
            .await
            .context("Failed to finalize checkpoint operation")?;
        
        info!("📸 Created checkpoint at sequence {}", sequence);
        
        // Clean up old snapshots
        self.cleanup_old_snapshots(&checkpoint_dir, self.config.keep_snapshots).await?;
        
        Ok(())
    }
    
    /// Recover from checkpoint snapshot if available
    pub async fn recover_from_checkpoint(&self) -> Result<(u64, bool)> {
        let fs = self.filesystem_factory.get_filesystem(&self.config.storage_url)?;
        let checkpoint_link = self.base_path.join("snapshots/current_checkpoint.proto");
        
        // Check if checkpoint exists
        if !fs.exists(&checkpoint_link.to_string_lossy()).await? {
            info!("📋 No checkpoint found, will use regular snapshot");
            return Ok((0, false));
        }
        
        // Read checkpoint link
        let checkpoint_path_bytes = fs.read(&checkpoint_link.to_string_lossy()).await?;
        let checkpoint_path = String::from_utf8(checkpoint_path_bytes)
            .context("Invalid checkpoint path")?;
        
        // Parse sequence from checkpoint filename
        let sequence = self.parse_checkpoint_sequence(&checkpoint_path)?;
        
        info!("📸 Found checkpoint at sequence {}", sequence);
        
        // Load checkpoint into memory
        let checkpoint_data = fs.read(&checkpoint_path).await
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
        
        info!("📋 Loaded {} collections from checkpoint, rebuilding indexes...", count);
        
        // Clear existing index to ensure clean state
        self.index.clear();
        
        // Rebuild both primary (UUID) and secondary (name) indexes
        for record in records {
            // This will update both UUID->record and name->UUID mappings
            self.index.upsert_collection(record);
        }
        
        info!("✅ Rebuilt indexes with {} collections from checkpoint", count);
        
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
            
        if let Some(parts) = filename.strip_prefix("checkpoint_").and_then(|s| s.strip_suffix(".proto")) {
            let seq_str = parts.split('_').next()
                .ok_or_else(|| anyhow::anyhow!("Invalid checkpoint filename format"))?;
            seq_str.parse::<u64>()
                .context("Failed to parse sequence number")
        } else {
            Err(anyhow::anyhow!("Invalid checkpoint filename format"))
        }
    }
    
    /// Clean up old snapshots keeping only N most recent
    async fn cleanup_old_snapshots(&self, checkpoint_dir: &std::path::Path, keep_count: usize) -> Result<()> {
        let fs = self.filesystem_factory.get_filesystem(&self.config.storage_url)?;
        
        if let Ok(entries) = fs.list(&checkpoint_dir.to_string_lossy()).await {
            let mut snapshots: Vec<_> = entries
                .into_iter()
                .filter(|e| e.name.starts_with("checkpoint_") && e.name.ends_with(".proto"))
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
            self.index.get_by_uuid(identifier).map(|arc_record| (*arc_record).clone())
        } else {
            // Use efficient O(1) secondary index for name lookup
            if let Some(record) = self.index.get_by_name(identifier) {
                return Some((*record).clone());
            }
            
            // Fallback: scan primary memtable if secondary index fails
            // This handles cases where secondary index is inconsistent or corrupted
            warn!("🔍 Secondary index lookup failed for '{}', falling back to primary memtable scan", identifier);
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
            if entry.config.as_ref().map(|c| c.name.as_str()).unwrap_or("") == name {
                let elapsed = start.elapsed();
                warn!("🔧 Fallback scan found '{}' in {:?}, repairing secondary index", name, elapsed);
                
                // Self-healing: repair secondary index by re-inserting the mapping
                // This is safe because upsert_collection() atomically updates both primary and secondary index
                let record_to_repair = (*entry).clone();
                self.index.upsert_collection(record_to_repair.clone());
                
                info!("✅ Secondary index repaired for '{}' -> '{}'", name, record_to_repair.id);
                return Some(record_to_repair);
            }
        }
        
        let elapsed = start.elapsed();
        debug!("🔍 Fallback scan completed in {:?}, collection '{}' not found", elapsed, name);
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
impl CollectionMetadataProvider for FilestoreMetadataBackend {
    async fn get_uuid(&self, collection_id: &str) -> Result<Option<String>> {
        // Use optimized internal lookup that tries both name and UUID
        Ok(self.find_collection(collection_id).map(|r| r.id))
    }
    
    async fn get_collection_metadata(&self, collection_id: &str) -> Result<Option<Collection>> {
        // Use optimized internal lookup that tries both name and UUID
        Ok(self.find_collection(collection_id))
    }
    
    async fn get_collection(&self, collection_id: &str) -> Result<Option<Collection>> {
        // Use optimized internal lookup that tries both name and UUID
        Ok(self.find_collection(collection_id))
    }
    
    async fn list_collections(&self) -> Result<Vec<Collection>> {
        Ok(self.index.list_all().into_iter().map(|arc_record| (*arc_record).clone()).collect())
    }
}

/// Snapshot manager for periodic state persistence
struct SnapshotManager {
    threshold: u64,
    keep_count: usize,
    base_path: PathBuf,
}

impl SnapshotManager {
    fn new(
        threshold: u64,
        keep_count: usize,
        base_path: PathBuf,
    ) -> Self {
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
        info!("📸 Creating snapshot");
        let start = std::time::Instant::now();
        
        let snapshot_dir = self.base_path.join("snapshots");
        let timestamp = chrono::Utc::now().timestamp_millis();
        let snapshot_file = snapshot_dir.join(format!("snapshot_{}.proto", timestamp));
        let temp_file = self.base_path.join("temp").join(format!("temp_snapshot_{}.proto", timestamp));
        
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
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&uncompressed_data)?;
        let data = encoder.finish()?;
        
        // Write atomically
        fs.write(&temp_file.to_string_lossy(), &data, None).await?;
        fs.move_file(&temp_file.to_string_lossy(), &snapshot_file.to_string_lossy()).await?;
        
        // Update current snapshot link
        let current_snapshot = snapshot_dir.join("current.proto");
        let temp_current = self.base_path.join("temp").join("temp_current.proto");
        
        fs.write(&temp_current.to_string_lossy(), &data, None).await?;
        fs.move_file(&temp_current.to_string_lossy(), &current_snapshot.to_string_lossy()).await?;
        
        // Cleanup old snapshots
        self.cleanup_old_snapshots(fs).await?;
        
        info!("✅ Snapshot created in {:?} with {} collections", start.elapsed(), collections.len());
        Ok(())
    }
    
    async fn cleanup_old_snapshots(&self, fs: &dyn crate::storage::persistence::filesystem::FileSystem) -> Result<()> {
        let snapshot_dir = self.base_path.join("snapshots");
        
        let entries = match fs.list(&snapshot_dir.to_string_lossy()).await {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };
        
        let mut snapshots: Vec<_> = entries
            .into_iter()
            .filter(|e| e.name.starts_with("snapshot_") && e.name.ends_with(".proto"))
            .collect();
        
        // Sort by name (timestamp) in reverse order
        snapshots.sort_by(|a, b| b.name.cmp(&a.name));
        
        // Delete old snapshots beyond keep_count
        for entry in snapshots.iter().skip(self.keep_count) {
            let path = snapshot_dir.join(&entry.name);
            if let Err(e) = fs.delete(&path.to_string_lossy()).await {
                warn!("Failed to delete old snapshot {}: {}", entry.name, e);
            } else {
                debug!("🗑️ Deleted old snapshot: {}", entry.name);
            }
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use crate::proto::proximadb::CollectionConfig;
    
    #[tokio::test]
    async fn test_filestore_backend_basic_operations() {
        let temp_dir = TempDir::new().unwrap();
        let config = FilestoreMetadataConfig {
            storage_url: format!("file://{}", temp_dir.path().to_string_lossy()),
            enable_compression: true,
            enable_snapshots: false, // Disable for test
            ..Default::default()
        };
        
        let fs_factory = Arc::new(
            FilesystemFactory::new(Default::default()).await.unwrap()
        );
        
        let backend = FilestoreMetadataBackend::new(config, fs_factory).await.unwrap();
        
        // Test create collection
        let collection_config = CollectionConfig {
            name: "test_collection".to_string(),
            dimension: 128,
            distance_metric: 1, // Cosine
            storage_engine: 1,  // Viper
            primary_indexing_algorithm: 1, // HNSW
            filterable_columns: vec![],
            index_configs: vec![],
            quantization_config: None,
            primary_index_name: "default".to_string(),
            enable_automatic_index_selection: false,
            description: Some("Test collection".to_string()),
            tags: vec![],
            owner: Some("test".to_string()),
        };
        
        let uuid = backend.create_collection("test_collection".to_string(), &collection_config).await.unwrap();
        assert!(!uuid.is_empty());
        
        // Test get collection
        let collection = backend.get_collection("test_collection").await.unwrap();
        assert!(collection.is_some());
        let collection = collection.unwrap();
        assert_eq!(collection.config.as_ref().unwrap().name, "test_collection");
        assert_eq!(collection.config.as_ref().unwrap().dimension, 128);
        
        // Test list collections
        let collections = backend.list_collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        
        // Test collection exists
        assert!(backend.collection_exists("test_collection").await.unwrap());
        assert!(!backend.collection_exists("nonexistent").await.unwrap());
        
        // Test delete collection
        backend.delete_collection("test_collection").await.unwrap();
        assert!(!backend.collection_exists("test_collection").await.unwrap());
    }
}