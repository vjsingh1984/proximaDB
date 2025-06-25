// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Filestore-based Metadata Backend using Avro Schema
//!
//! This backend provides persistent metadata storage completely decoupled from WAL.
//! It uses pure Avro records with a snapshot + incremental log pattern.
//!
//! ## Architecture:
//! - Two separate Avro schemas: CollectionRecord (data) and IncrementalOperation (log)
//! - Sequential ordering using sequence numbers (reset to 0 on restart)
//! - Atomic recovery with compaction on restart
//! - UUID-based storage organization
//! - Archive management keeping last 5 snapshots
//!
//! ## File Structure:
//! ```
//! {filestore_url}/metadata/
//!   ├── snapshots/
//!   │   └── current_collections.avro
//!   ├── incremental/
//!   │   ├── op_00000001_20250619120000.avro
//!   │   ├── op_00000002_20250619120100.avro
//!   │   └── ...
//!   └── archive/
//!       ├── 20250619_120000/
//!       │   ├── snapshot_collections.avro
//!       │   └── incremental/
//!       └── ...
//! ```

use anyhow::Result;
use apache_avro::{Schema, Writer, Reader, Codec};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::info;
use uuid::Uuid;

use crate::proto::proximadb::CollectionConfig;
use crate::storage::persistence::filesystem::{FilesystemFactory, atomic_strategy::{AtomicWriteExecutor, AtomicWriteExecutorFactory, AtomicWriteConfig}};
use crate::storage::metadata::single_index::{SingleCollectionIndex, SingleIndexMetrics};

/// Canonical Avro schema for collection metadata - tightly coupled to filestore backend
pub const COLLECTION_AVRO_SCHEMA: &str = r#"
{
  "type": "record",
  "name": "CollectionRecord",
  "namespace": "ai.proximadb.filestore",
  "doc": "Collection metadata record",
  "fields": [
    {"name": "uuid", "type": "string"},
    {"name": "name", "type": "string"},
    {"name": "dimension", "type": "int"},
    {"name": "distance_metric", "type": "string"},
    {"name": "indexing_algorithm", "type": "string"},
    {"name": "storage_engine", "type": "string"},
    {"name": "created_at", "type": "long"},
    {"name": "updated_at", "type": "long"},
    {"name": "version", "type": "long", "default": 1},
    {"name": "vector_count", "type": "long", "default": 0},
    {"name": "total_size_bytes", "type": "long", "default": 0},
    {"name": "config", "type": "string", "default": "{}"},
    {"name": "description", "type": ["null", "string"], "default": null},
    {"name": "tags", "type": {"type": "array", "items": "string"}, "default": []},
    {"name": "owner", "type": ["null", "string"], "default": null}
  ]
}
"#;

/// Avro schema for incremental operations log
pub const INCREMENTAL_OPERATION_AVRO_SCHEMA: &str = r#"
{
  "type": "record",
  "name": "IncrementalOperation",
  "namespace": "ai.proximadb.filestore",
  "doc": "Incremental operation for sequential replay",
  "fields": [
    {"name": "operation_type", "type": "string"},
    {"name": "sequence_number", "type": "long"},
    {"name": "timestamp", "type": "string"},
    {"name": "collection_id", "type": "string"},
    {"name": "collection_data", "type": ["null", "string"], "default": null}
  ]
}
"#;

/// Pure Avro collection record
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectionRecord {
    pub uuid: String,
    pub name: String,
    pub dimension: i32,
    pub distance_metric: String,
    pub indexing_algorithm: String,
    pub storage_engine: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
    pub vector_count: i64,
    pub total_size_bytes: i64,
    pub config: String, // JSON serialized
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
}

impl CollectionRecord {
    /// Create from gRPC CollectionConfig
    pub fn from_grpc_config(name: String, config: &CollectionConfig) -> Result<Self> {
        let uuid = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        
        let distance_metric = match config.distance_metric {
            1 => "COSINE",
            2 => "EUCLIDEAN", 
            3 => "DOT_PRODUCT",
            4 => "HAMMING",
            _ => "COSINE",
        }.to_string();
        
        let indexing_algorithm = match config.indexing_algorithm {
            1 => "HNSW",
            2 => "IVF",
            3 => "PQ",
            4 => "FLAT",
            5 => "ANNOY",
            _ => "HNSW",
        }.to_string();
        
        let storage_engine = match config.storage_engine {
            1 => "VIPER",
            2 => "LSM",
            3 => "MMAP",
            4 => "HYBRID",
            _ => "VIPER",
        }.to_string();
        
        // Convert indexing config to JSON
        let config_json = serde_json::to_string(&config.indexing_config)?;
        
        Ok(Self {
            uuid,
            name,
            dimension: config.dimension,
            distance_metric,
            indexing_algorithm,
            storage_engine,
            created_at: now,
            updated_at: now,
            version: 1,
            vector_count: 0,
            total_size_bytes: 0,
            config: config_json,
            description: None,
            tags: vec![],
            owner: None,
        })
    }
    
    /// Convert to gRPC CollectionConfig
    pub fn to_grpc_config(&self) -> CollectionConfig {
        let distance_metric = match self.distance_metric.as_str() {
            "COSINE" => 1,
            "EUCLIDEAN" => 2,
            "DOT_PRODUCT" => 3,
            "HAMMING" => 4,
            _ => 1,
        };
        
        let indexing_algorithm = match self.indexing_algorithm.as_str() {
            "HNSW" => 1,
            "IVF" => 2,
            "PQ" => 3,
            "FLAT" => 4,
            "ANNOY" => 5,
            _ => 1,
        };
        
        let storage_engine = match self.storage_engine.as_str() {
            "VIPER" => 1,
            "LSM" => 2,
            "MMAP" => 3,
            "HYBRID" => 4,
            _ => 1,
        };
        
        // Parse indexing config from JSON
        let indexing_config = serde_json::from_str(&self.config)
            .unwrap_or_else(|_| std::collections::HashMap::new());
        
        CollectionConfig {
            name: self.name.clone(),
            dimension: self.dimension,
            distance_metric,
            indexing_algorithm,
            storage_engine,
            filterable_metadata_fields: vec![],
            indexing_config,
            filterable_columns: Vec::new(), // Empty by default for new filterable column API
        }
    }
    
    /// Update statistics
    pub fn update_stats(&mut self, vector_delta: i64, size_delta: i64) {
        self.vector_count = (self.vector_count + vector_delta).max(0);
        self.total_size_bytes = (self.total_size_bytes + size_delta).max(0);
        self.updated_at = Utc::now().timestamp_millis();
        self.version += 1;
    }
    
    /// Get storage path template
    pub fn storage_path(&self, base_path: &str) -> String {
        format!("{}/collections/{}/{}", base_path, &self.uuid[0..2], self.uuid)
    }
}

/// Operation types for incremental log
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OperationType {
    Insert,
    Update,
    Delete,
}

/// Incremental operation record for sequential replay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalOperation {
    pub operation_type: OperationType,
    pub sequence_number: u64,
    pub timestamp: String, // YYYYMMDDHH24MISS format
    pub collection_id: String, // UUID
    pub collection_data: Option<CollectionRecord>,
}

impl IncrementalOperation {
    /// Create insert operation
    pub fn insert(sequence: u64, record: CollectionRecord) -> Self {
        Self {
            operation_type: OperationType::Insert,
            sequence_number: sequence,
            timestamp: Utc::now().format("%Y%m%d%H%M%S").to_string(),
            collection_id: record.uuid.clone(),
            collection_data: Some(record),
        }
    }
    
    /// Create update operation
    pub fn update(sequence: u64, record: CollectionRecord) -> Self {
        Self {
            operation_type: OperationType::Update,
            sequence_number: sequence,
            timestamp: Utc::now().format("%Y%m%d%H%M%S").to_string(),
            collection_id: record.uuid.clone(),
            collection_data: Some(record),
        }
    }
    
    /// Create delete operation
    pub fn delete(sequence: u64, collection_id: String) -> Self {
        Self {
            operation_type: OperationType::Delete,
            sequence_number: sequence,
            timestamp: Utc::now().format("%Y%m%d%H%M%S").to_string(),
            collection_id,
            collection_data: None,
        }
    }
}

/// Filestore backend configuration
#[derive(Debug, Clone)]
pub struct FilestoreMetadataConfig {
    pub filestore_url: String,
    pub enable_compression: bool,
    pub enable_backup: bool,
    pub enable_snapshot_archival: bool,
    pub max_archived_snapshots: usize,
    
    /// Optional temp directory for atomic operations
    /// If None, uses system temp or same-directory strategy
    pub temp_directory: Option<String>,
}

impl Default for FilestoreMetadataConfig {
    fn default() -> Self {
        Self {
            filestore_url: "file://./data".to_string(),
            enable_compression: true,
            enable_backup: true,
            enable_snapshot_archival: true,
            max_archived_snapshots: 5,
            temp_directory: None, // Uses same-directory strategy by default
        }
    }
}

/// Filestore metadata backend implementation with unified index and atomic writes
pub struct FilestoreMetadataBackend {
    config: FilestoreMetadataConfig,
    filesystem: Arc<FilesystemFactory>,
    filestore_url: String,
    metadata_path: PathBuf,
    
    // Single unified index - eliminates dual-index sync complexity
    single_index: SingleCollectionIndex,
    
    // Atomic write executor for robust persistence
    #[allow(dead_code)]
    atomic_writer: Box<dyn AtomicWriteExecutor>,
    
    // Recovery lock - blocks all API operations during recovery
    recovery_lock: Arc<tokio::sync::RwLock<()>>,
    
    // Sequence counter for incremental operations (reset to 0 on restart)
    sequence_counter: Arc<AtomicU64>,
    
    // Avro schemas
    collection_schema: Schema,
    operation_schema: Schema,
}

impl std::fmt::Debug for FilestoreMetadataBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilestoreMetadataBackend")
            .field("config", &self.config)
            .field("filestore_url", &self.filestore_url)
            .field("metadata_path", &self.metadata_path)
            .field("single_index", &self.single_index)
            .field("atomic_writer", &"<AtomicWriteExecutor>")
            .field("recovery_lock", &"<RwLock>")
            .field("sequence_counter", &self.sequence_counter)
            .finish()
    }
}

impl FilestoreMetadataBackend {
    /// Create new filestore metadata backend
    pub async fn new(
        config: FilestoreMetadataConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        info!("🗃️ Creating FilestoreMetadataBackend");
        info!("📁 Filestore URL: {}", config.filestore_url);
        
        // Parse Avro schemas
        let collection_schema = Schema::parse_str(COLLECTION_AVRO_SCHEMA)?;
        let operation_schema = Schema::parse_str(INCREMENTAL_OPERATION_AVRO_SCHEMA)?;
        
        // FilestoreMetadataBackend works within the filesystem abstraction
        // The filesystem layer handles URL parsing and storage strategy
        // We use empty PathBuf since filesystem already handles base path resolution
        let metadata_path = PathBuf::new();
        
        info!("📁 FilestoreMetadataBackend: Working in root directory specified by URL: {}", config.filestore_url);
        
        // Ensure required subdirectories exist in the metadata root
        let fs = filesystem.get_filesystem(&config.filestore_url)?;
        fs.create_dir("snapshots").await.ok();
        fs.create_dir("incremental").await.ok();
        fs.create_dir("archive").await.ok();
        
        // Create atomic write executor based on environment
        let atomic_config = AtomicWriteConfig::default();
        let atomic_writer = if cfg!(test) {
            // Use direct writes for tests (faster)
            AtomicWriteExecutorFactory::create_dev_executor()
        } else {
            // Use production-grade atomic writes
            AtomicWriteExecutorFactory::create_executor(&atomic_config)
        };
        
        let backend = Self {
            filestore_url: config.filestore_url.clone(),
            metadata_path,
            config,
            filesystem,
            single_index: SingleCollectionIndex::new(),
            atomic_writer,
            recovery_lock: Arc::new(tokio::sync::RwLock::new(())),
            sequence_counter: Arc::new(AtomicU64::new(0)),
            collection_schema,
            operation_schema,
        };
        
        // Perform recovery
        backend.recover_from_snapshot_and_incremental().await?;
        
        info!("✅ FilestoreMetadataBackend initialized");
        Ok(backend)
    }
    
    /// Atomic recovery process
    async fn recover_from_snapshot_and_incremental(&self) -> Result<()> {
        info!("🔄 Starting ATOMIC recovery...");
        info!("📂 Filestore URL: {}", self.filestore_url);
        info!("📁 Metadata path: {}", self.metadata_path.display());
        let start = std::time::Instant::now();
        
        // Acquire exclusive lock
        let _guard = self.recovery_lock.write().await;
        info!("🔒 Recovery lock acquired - API requests blocked");
        
        // Reset sequence counter
        self.sequence_counter.store(0, Ordering::SeqCst);
        
        // Load snapshot
        info!("📸 Loading latest snapshot...");
        let mut memtable = self.load_latest_snapshot().await?;
        info!("📸 Loaded {} collections from snapshot", memtable.len());
        for (uuid, record) in &memtable {
            info!("   📋 Snapshot collection: {} (UUID: {})", record.name, uuid);
        }
        
        // Replay incremental operations
        info!("🔄 Replaying incremental operations...");
        let ops_count = self.replay_incremental_operations(&mut memtable).await?;
        info!("🔄 Replayed {} operations", ops_count);
        info!("📊 Final memtable size: {}", memtable.len());
        for (uuid, record) in &memtable {
            info!("   📋 Final collection: {} (UUID: {})", record.name, uuid);
        }
        
        // Update single index - no sync issues, single operation
        let records: Vec<CollectionRecord> = memtable.clone().into_values().collect();
        self.single_index.rebuild_from_records(records);
        
        // Archive old files
        if self.config.enable_snapshot_archival {
            self.archive_current_state().await?;
        }
        
        // Create new snapshot
        self.create_new_snapshot(&memtable).await?;
        
        info!("✅ Recovery completed in {:?}", start.elapsed());
        Ok(())
    }
    
    /// Load latest snapshot
    async fn load_latest_snapshot(&self) -> Result<BTreeMap<String, CollectionRecord>> {
        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;
        let snapshot_path = self.metadata_path.join("snapshots/current_collections.avro");
        info!("📁 Snapshot path: {}", snapshot_path.display());
        
        let mut memtable = BTreeMap::new();
        
        if fs.exists(&snapshot_path.to_string_lossy()).await? {
            info!("📄 Snapshot file exists, reading...");
            let data = fs.read(&snapshot_path.to_string_lossy()).await?;
            info!("📊 Snapshot file size: {} bytes", data.len());
            
            // Check if data is not empty before creating Avro reader
            if !data.is_empty() {
                let reader = Reader::new(&data[..])?;
                info!("📖 Created Avro reader, processing records...");
                
                let mut count = 0;
                for value in reader {
                    let record: CollectionRecord = apache_avro::from_value(&value?)?;
                    info!("   📋 Loaded snapshot record: {} (UUID: {})", record.name, record.uuid);
                    memtable.insert(record.uuid.clone(), record);
                    count += 1;
                }
                info!("📊 Loaded {} records from snapshot", count);
            } else {
                info!("📭 Snapshot file is empty");
            }
        } else {
            info!("❌ No snapshot file found at: {}", snapshot_path.display());
        }
        
        Ok(memtable)
    }
    
    /// Replay incremental operations in sequential order
    async fn replay_incremental_operations(&self, memtable: &mut BTreeMap<String, CollectionRecord>) -> Result<usize> {
        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;
        let incremental_dir = self.metadata_path.join("incremental");
        
        let mut operations = Vec::new();
        
        // Read all operation files
        if let Ok(entries) = fs.list(&incremental_dir.to_string_lossy()).await {
            for entry in entries {
                if entry.name.starts_with("op_") && entry.name.ends_with(".avro") {
                    let path = incremental_dir.join(&entry.name);
                    if let Ok(data) = fs.read(&path.to_string_lossy()).await {
                        // Check if data is not empty before creating Avro reader
                        if !data.is_empty() {
                            let reader = Reader::new(&data[..])?;
                            
                            // Define the AvroOperation struct that matches what was written
                            #[derive(Deserialize)]
                            struct AvroOperationRead {
                                operation_type: String,
                                sequence_number: i64,
                                timestamp: String,
                                collection_id: String,
                                collection_data: Option<String>,
                            }
                            
                            for value in reader {
                                if let Ok(avro_value) = value {
                                    // Use proper Avro deserialization instead of manual parsing
                                    if let Ok(avro_op) = apache_avro::from_value::<AvroOperationRead>(&avro_value) {
                                        let op_type = match avro_op.operation_type.as_str() {
                                            "Insert" => OperationType::Insert,
                                            "Update" => OperationType::Update,
                                            "Delete" => OperationType::Delete,
                                            _ => {
                                                info!("⚠️ Unknown operation type: {}, skipping", avro_op.operation_type);
                                                continue;
                                            }
                                        };
                                        
                                        let collection_data = avro_op.collection_data
                                            .and_then(|json| {
                                                match serde_json::from_str::<CollectionRecord>(&json) {
                                                    Ok(record) => {
                                                        info!("✅ Successfully parsed collection data for: {}", record.name);
                                                        Some(record)
                                                    },
                                                    Err(e) => {
                                                        info!("❌ Failed to parse collection_data JSON: {}", e);
                                                        info!("📄 Raw JSON: {}", &json[..json.len().min(200)]);
                                                        None
                                                    }
                                                }
                                            });
                                        
                                        let operation = IncrementalOperation {
                                            operation_type: op_type,
                                            sequence_number: avro_op.sequence_number as u64,
                                            timestamp: avro_op.timestamp,
                                            collection_id: avro_op.collection_id.clone(),
                                            collection_data,
                                        };
                                        
                                        info!("📋 Parsed operation: type={:?}, seq={}, id={}, has_data={}", 
                                              operation.operation_type, operation.sequence_number, 
                                              operation.collection_id, operation.collection_data.is_some());
                                        
                                        operations.push(operation);
                                    } else {
                                        info!("❌ Failed to deserialize Avro operation from value");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        // Sort operations by sequence number and timestamp
        operations.sort_by(|a, b| {
            a.sequence_number.cmp(&b.sequence_number)
                .then_with(|| a.timestamp.cmp(&b.timestamp))
        });
        
        // Apply operations to memtable
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
        
        Ok(operations.len())
    }
    
    /// Archive current state
    async fn archive_current_state(&self) -> Result<()> {
        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let archive_dir = self.metadata_path.join(format!("archive/{}", timestamp));
        
        // Create archive directory
        fs.create_dir(&archive_dir.to_string_lossy()).await?;
        
        // Move current snapshot if exists
        let current_snapshot = self.metadata_path.join("snapshots/current_collections.avro");
        if fs.exists(&current_snapshot.to_string_lossy()).await? {
            let archive_snapshot = archive_dir.join("snapshot_collections.avro");
            fs.copy(&current_snapshot.to_string_lossy(), &archive_snapshot.to_string_lossy()).await?;
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
                    fs.copy(&src.to_string_lossy(), &dst.to_string_lossy()).await?;
                    fs.delete(&src.to_string_lossy()).await?;
                }
            }
        }
        
        // Clean up old archives
        self.cleanup_old_archives().await?;
        
        Ok(())
    }
    
    /// Fast atomic write using pre-computed write strategy (performance optimized)
    async fn write_atomically(&self, fs: &dyn crate::storage::persistence::filesystem::FileSystem, 
                             final_path: &std::path::Path, data: &[u8]) -> Result<()> {
        use crate::storage::persistence::filesystem::write_strategy::{WriteStrategyFactory};
        
        // Create metadata-optimized write strategy (computed once, not per write)
        let strategy = WriteStrategyFactory::create_metadata_strategy(fs, self.config.temp_directory.as_deref())?;
        
        // Generate optimized file options for this specific file
        let file_options = strategy.create_file_options(fs, &final_path.to_string_lossy())?;
        
        // Use fast atomic write with pre-computed options
        info!("💾 Atomic write with optimized strategy: {}", final_path.display());
        fs.write_atomic(&final_path.to_string_lossy(), data, Some(file_options)).await?;
        info!("✅ Optimized atomic write completed");
        
        Ok(())
    }

    /// Create new snapshot
    async fn create_new_snapshot(&self, memtable: &BTreeMap<String, CollectionRecord>) -> Result<()> {
        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;
        let snapshot_path = self.metadata_path.join("snapshots/current_collections.avro");
        
        // Create writer
        let codec = if self.config.enable_compression {
            Codec::Deflate
        } else {
            Codec::Null
        };
        
        let mut writer = Writer::with_codec(&self.collection_schema, Vec::new(), codec);
        
        // Write all records
        for record in memtable.values() {
            writer.append_ser(record)?;
        }
        
        let data = writer.into_inner()?;
        
        // Write with optimal strategy
        self.write_atomically(fs, &snapshot_path, &data).await?;
        
        Ok(())
    }
    
    /// Clean up old archives
    async fn cleanup_old_archives(&self) -> Result<()> {
        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;
        let archive_dir = self.metadata_path.join("archive");
        
        if let Ok(entries) = fs.list(&archive_dir.to_string_lossy()).await {
            let mut dirs: Vec<_> = entries.into_iter()
                .filter(|e| e.metadata.is_directory)
                .map(|e| e.name)
                .collect();
            
            dirs.sort();
            
            // Remove oldest if we have too many
            while dirs.len() > self.config.max_archived_snapshots {
                if let Some(oldest) = dirs.first() {
                    let path = archive_dir.join(oldest);
                    fs.delete(&path.to_string_lossy()).await.ok();
                    dirs.remove(0);
                }
            }
        }
        
        Ok(())
    }
    
    /// Write incremental operation
    async fn write_incremental_operation(&self, operation: IncrementalOperation) -> Result<()> {
        info!("💾 Writing incremental operation: seq={}, type={:?}, id={}", 
              operation.sequence_number, operation.operation_type, operation.collection_id);
        
        let fs = self.filesystem.get_filesystem(&self.filestore_url)?;
        info!("🔗 Got filesystem for URL: {}", self.filestore_url);
        
        // Create filename with sequence number
        let filename = format!("op_{:08}_{}.avro", 
            operation.sequence_number, 
            operation.timestamp);
        let path = self.metadata_path.join(format!("incremental/{}", filename));
        
        info!("📁 Writing to path: {}", path.display());
        
        // Serialize operation
        let mut writer = Writer::with_codec(&self.operation_schema, Vec::new(), Codec::Null);
        
        // Create a properly typed struct for Avro serialization
        #[derive(Serialize)]
        struct AvroOperation {
            operation_type: String,
            sequence_number: i64,
            timestamp: String,
            collection_id: String,
            collection_data: Option<String>,
        }
        
        let avro_op = AvroOperation {
            operation_type: match operation.operation_type {
                OperationType::Insert => "Insert".to_string(),
                OperationType::Update => "Update".to_string(),
                OperationType::Delete => "Delete".to_string(),
            },
            sequence_number: operation.sequence_number as i64,
            timestamp: operation.timestamp,
            collection_id: operation.collection_id,
            collection_data: operation.collection_data
                .map(|r| serde_json::to_string(&r).unwrap_or_default()),
        };
        
        writer.append_ser(&avro_op)?;
        let data = writer.into_inner()?;
        info!("📊 Serialized {} bytes of Avro data", data.len());
        
        // Write with optimal strategy
        self.write_atomically(fs, &path, &data).await?;
        
        Ok(())
    }
    
    // Public API methods
    
    /// Create or update collection
    pub async fn upsert_collection_record(&self, record: CollectionRecord) -> Result<()> {
        info!("🆕 Upserting collection record: {} (UUID: {})", record.name, record.uuid);
        
        // Acquire read lock (no recovery in progress)
        let _guard = self.recovery_lock.read().await;
        info!("🔒 Recovery lock acquired for upsert");
        
        // Get next sequence number
        let sequence = self.sequence_counter.fetch_add(1, Ordering::SeqCst);
        info!("🔢 Sequence number: {}", sequence);
        
        // Check if this is insert or update
        let is_update = self.single_index.exists_by_uuid(&record.uuid);
        
        let operation_type = if is_update { "UPDATE" } else { "INSERT" };
        info!("🔄 Operation type: {}", operation_type);
        
        // Create operation
        let operation = if is_update {
            IncrementalOperation::update(sequence, record.clone())
        } else {
            IncrementalOperation::insert(sequence, record.clone())
        };
        
        // Write to incremental log
        info!("💾 About to call write_incremental_operation...");
        self.write_incremental_operation(operation).await?;
        info!("✅ write_incremental_operation completed");
        
        // Update single index (single atomic operation - no sync complexity)
        info!("🧠 Updating single index...");
        self.single_index.upsert_collection(record);
        info!("✅ Single index updated");
        
        info!("🎉 upsert_collection_record completed successfully");
        Ok(())
    }
    
    /// Get collection by name - O(1) lookup using unified index
    pub async fn get_collection_record_by_name(&self, name: &str) -> Result<Option<CollectionRecord>> {
        let _guard = self.recovery_lock.read().await;
        
        Ok(self.single_index.get_by_name(name).map(|arc_record| (*arc_record).clone()))
    }
    
    /// Get collection UUID by name - O(1) lookup optimized for storage operations
    pub async fn get_collection_uuid_string(&self, name: &str) -> Result<Option<String>> {
        let _guard = self.recovery_lock.read().await;
        
        Ok(self.single_index.get_uuid_by_name(name))
    }
    
    /// Delete collection by name
    pub async fn delete_collection_by_name(&self, name: &str) -> Result<bool> {
        let _guard = self.recovery_lock.read().await;
        
        // Find UUID using single index
        let uuid = match self.single_index.get_uuid_by_name(name) {
            Some(uuid) => uuid,
            None => return Ok(false),
        };
        
        // Get next sequence number
        let sequence = self.sequence_counter.fetch_add(1, Ordering::SeqCst);
        
        // Create delete operation
        let operation = IncrementalOperation::delete(sequence, uuid.clone());
        
        // Write to incremental log
        self.write_incremental_operation(operation).await?;
        
        // Update single index (single atomic operation - no sync issues)
        self.single_index.remove_collection(&uuid);
        
        Ok(true)
    }
    
    /// List all collections - O(n) but efficient iteration
    pub async fn list_collections(&self, filter: Option<Box<dyn Fn(&CollectionRecord) -> bool + Send + Sync>>) -> Result<Vec<CollectionRecord>> {
        let _guard = self.recovery_lock.read().await;
        
        let collections: Vec<_> = if let Some(filter_fn) = filter {
            // Apply filter if provided
            self.single_index.filter_collections(|record| filter_fn(record))
                .into_iter()
                .map(|arc_record| (*arc_record).clone())
                .collect()
        } else {
            // Return all collections
            self.single_index.list_all()
                .into_iter()
                .map(|arc_record| (*arc_record).clone())
                .collect()
        };
        
        // Sort by name for consistent ordering
        let mut sorted_collections = collections;
        sorted_collections.sort_by(|a, b| a.name.cmp(&b.name));
        
        Ok(sorted_collections)
    }
    
    /// Get performance metrics from single index
    pub async fn get_index_metrics(&self) -> SingleIndexMetrics {
        self.single_index.get_metrics()
    }
}