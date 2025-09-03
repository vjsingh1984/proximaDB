// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! RocksDB-based metadata backend for high-performance persistent storage
//!
//! This backend provides:
//! - High-performance key-value storage with SST tree architecture
//! - ACID transactions with optimistic concurrency control
//! - Column families for organized data storage
//! - Built-in compression and bloom filters
//! - Efficient range queries and prefix scans
//! - Atomic batch operations
//! - Point-in-time backups and snapshots

use anyhow::{Context, Result};
use async_trait::async_trait;
use rocksdb::{
    BoundColumnFamily, ColumnFamilyDescriptor, DBWithThreadMode, IteratorMode, MultiThreaded,
    Options, ReadOptions, Transaction, TransactionDB, TransactionDBOptions, TransactionOptions,
    WriteBatch, WriteOptions,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::proto::proximadb::{
    Collection, CollectionConfig, CollectionStats, DistanceMetric, IndexingAlgorithm, StorageEngine,
};
use crate::storage::metadata::backends::MetadataBackend;
use crate::storage::metadata::single_index::SingleCollectionIndex;
use crate::storage::traits::CollectionMetadataProvider;
use prost::Message;

/// RocksDB column families for different data types
const CF_COLLECTIONS: &str = "collections";
const CF_UUID_INDEX: &str = "uuid_index";
const CF_NAME_INDEX: &str = "name_index";
const CF_METADATA: &str = "metadata_info";
const CF_STATS: &str = "statistics";
const CF_TAGS: &str = "tags";

/// RocksDB metadata backend configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RocksDbMetadataConfig {
    /// Database path
    pub db_path: PathBuf,

    /// Enable compression
    pub compression: bool,

    /// Use bloom filters for faster lookups
    pub use_bloom_filters: bool,

    /// Block cache size in MB
    pub block_cache_size_mb: usize,

    /// Write buffer size in MB
    pub write_buffer_size_mb: usize,

    /// Max open files
    pub max_open_files: i32,

    /// Enable statistics collection
    pub enable_statistics: bool,

    /// Compaction style (0: Level, 1: Universal, 2: FIFO)
    pub compaction_style: i32,

    /// Enable transactions
    pub enable_transactions: bool,

    /// Backup configuration
    pub backup_config: Option<RocksDbBackupConfig>,
}

impl Default for RocksDbMetadataConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from("./data/metadata/rocksdb"),
            compression: true,
            use_bloom_filters: true,
            block_cache_size_mb: 64,
            write_buffer_size_mb: 16,
            max_open_files: 1000,
            enable_statistics: true,
            compaction_style: 0, // Level compaction
            enable_transactions: true,
            backup_config: Some(RocksDbBackupConfig::default()),
        }
    }
}

/// RocksDB backup configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RocksDbBackupConfig {
    /// Backup directory path
    pub backup_path: PathBuf,

    /// Maximum number of backups to keep
    pub max_backups: u32,

    /// Enable incremental backups
    pub incremental: bool,

    /// Backup interval in seconds
    pub backup_interval_secs: u64,
}

impl Default for RocksDbBackupConfig {
    fn default() -> Self {
        Self {
            backup_path: PathBuf::from("./data/metadata/rocksdb_backups"),
            max_backups: 10,
            incremental: true,
            backup_interval_secs: 3600, // 1 hour
        }
    }
}

/// RocksDB-based metadata backend
pub struct RocksDbMetadataBackend {
    /// Configuration
    config: RocksDbMetadataConfig,

    /// RocksDB instance (TransactionDB if transactions enabled)
    db: Arc<RwLock<Option<Arc<TransactionDB>>>>,

    /// Regular DB instance (if transactions disabled)
    regular_db: Arc<RwLock<Option<Arc<DBWithThreadMode<MultiThreaded>>>>>,

    /// Statistics
    stats: Arc<RwLock<BackendStatistics>>,
}

/// Backend statistics
#[derive(Debug, Default)]
struct BackendStatistics {
    total_reads: u64,
    total_writes: u64,
    total_deletes: u64,
    cache_hits: u64,
    cache_misses: u64,
    bytes_written: u64,
    bytes_read: u64,
    compactions: u64,
}

impl RocksDbMetadataBackend {
    /// Create new RocksDB metadata backend
    pub async fn new(config: RocksDbMetadataConfig) -> Result<Self> {
        let backend = Self {
            config,
            db: Arc::new(RwLock::new(None)),
            regular_db: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(BackendStatistics::default())),
        };

        backend.initialize().await?;
        Ok(backend)
    }

    /// Initialize the database
    async fn initialize(&self) -> Result<()> {
        info!(
            "🗄️ Initializing RocksDB metadata backend at {:?}",
            self.config.db_path
        );

        // Create directory if it doesn't exist
        std::fs::create_dir_all(&self.config.db_path)
            .context("Failed to create RocksDB directory")?;

        // Configure column families
        let cf_descriptors = vec![
            ColumnFamilyDescriptor::new(CF_COLLECTIONS, Self::cf_options()),
            ColumnFamilyDescriptor::new(CF_UUID_INDEX, Self::cf_options()),
            ColumnFamilyDescriptor::new(CF_NAME_INDEX, Self::cf_options()),
            ColumnFamilyDescriptor::new(CF_METADATA, Self::cf_options()),
            ColumnFamilyDescriptor::new(CF_STATS, Self::cf_options()),
            ColumnFamilyDescriptor::new(CF_TAGS, Self::cf_options()),
        ];

        // Configure database options
        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_max_open_files(self.config.max_open_files);

        if self.config.enable_compression {
            db_opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
        }

        if self.config.use_bloom_filters {
            db_opts.set_prefix_extractor(rocksdb::SliceTransform::create_fixed_prefix(8));
        }

        // Set block cache
        let cache = rocksdb::Cache::new_lru_cache(self.config.block_cache_size_mb * 1024 * 1024);
        let mut block_opts = rocksdb::BlockBasedOptions::default();
        block_opts.set_block_cache(&cache);
        db_opts.set_block_based_table_factory(&block_opts);

        // Open database
        if self.config.enable_transactions {
            let txn_db_opts = TransactionDBOptions::default();
            let db = TransactionDB::open_cf_descriptors(
                &db_opts,
                &txn_db_opts,
                &self.config.db_path,
                cf_descriptors,
            )
            .context("Failed to open RocksDB with transactions")?;

            *self.db.write().await = Some(Arc::new(db));
            info!("✅ RocksDB initialized with transaction support");
        } else {
            let db = DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(
                &db_opts,
                &self.config.db_path,
                cf_descriptors,
            )
            .context("Failed to open RocksDB")?;

            *self.regular_db.write().await = Some(Arc::new(db));
            info!("✅ RocksDB initialized without transaction support");
        }

        Ok(())
    }

    /// Get column family options
    fn cf_options() -> Options {
        let mut opts = Options::default();
        opts.set_write_buffer_size(16 * 1024 * 1024); // 16MB
        opts.set_max_write_buffer_number(3);
        opts.set_target_file_size_base(64 * 1024 * 1024); // 64MB
        opts.set_level_compaction_dynamic_level_bytes(true);
        opts
    }

    /// Get column family handle
    fn get_cf(&self, db: &TransactionDB, cf_name: &str) -> Result<&rocksdb::ColumnFamily> {
        db.cf_handle(cf_name)
            .ok_or_else(|| anyhow::anyhow!("Column family {} not found", cf_name))
    }

    /// Serialize collection record to bytes
    fn serialize_record(record: &Collection) -> Result<Vec<u8>> {
        bincode::serialize(record).context("Failed to serialize collection record")
    }

    /// Deserialize collection record from bytes
    fn deserialize_record(data: &[u8]) -> Result<Collection> {
        bincode::deserialize(data).context("Failed to deserialize collection record")
    }

    /// Get database handle (either transactional or regular)
    async fn get_db(&self) -> Result<DbHandle> {
        if self.config.enable_transactions {
            let db_guard = self.db.read().await;
            if let Some(db) = db_guard.as_ref() {
                Ok(DbHandle::Transactional(db.clone()))
            } else {
                Err(anyhow::anyhow!("RocksDB not initialized"))
            }
        } else {
            let db_guard = self.regular_db.read().await;
            if let Some(db) = db_guard.as_ref() {
                Ok(DbHandle::Regular(db.clone()))
            } else {
                Err(anyhow::anyhow!("RocksDB not initialized"))
            }
        }
    }

    /// Update statistics
    async fn update_stats(&self, op: StatOp) {
        let mut stats = self.stats.write().await;
        match op {
            StatOp::Read(bytes) => {
                stats.total_reads += 1;
                stats.bytes_read += bytes;
            }
            StatOp::Write(bytes) => {
                stats.total_writes += 1;
                stats.bytes_written += bytes;
            }
            StatOp::Delete => {
                stats.total_deletes += 1;
            }
            StatOp::CacheHit => {
                stats.cache_hits += 1;
            }
            StatOp::CacheMiss => {
                stats.cache_misses += 1;
            }
            StatOp::Compaction => {
                stats.compactions += 1;
            }
        }
    }
}

/// Database handle enum for unified API
enum DbHandle {
    Transactional(Arc<TransactionDB>),
    Regular(Arc<DBWithThreadMode<MultiThreaded>>),
}

/// Statistics operation type
enum StatOp {
    Read(u64),
    Write(u64),
    Delete,
    CacheHit,
    CacheMiss,
    Compaction,
}

#[async_trait]
impl CollectionMetadataProvider for RocksDbMetadataBackend {
    async fn get_uuid(&self, collection_id: &str) -> Result<Option<String>> {
        let db = self.get_db().await?;

        match db {
            DbHandle::Transactional(db) => {
                let cf = self.get_cf(&db, CF_NAME_INDEX)?;
                let result = db.get_cf(&cf, collection_id.as_bytes());
                drop(cf); // Release CF reference before await
                match result {
                    Ok(Some(uuid_bytes)) => {
                        let len = uuid_bytes.len() as u64;
                        self.update_stats(StatOp::Read(len)).await;
                        Ok(Some(String::from_utf8(uuid_bytes)?))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(anyhow::anyhow!("Failed to get UUID: {}", e)),
                }
            }
            DbHandle::Regular(db) => {
                let cf = db
                    .cf_handle(CF_NAME_INDEX)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                let result = db.get_cf(&cf, collection_id.as_bytes());
                match result {
                    Ok(Some(uuid_bytes)) => {
                        let len = uuid_bytes.len() as u64;
                        self.update_stats(StatOp::Read(len)).await;
                        Ok(Some(String::from_utf8(uuid_bytes)?))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(anyhow::anyhow!("Failed to get UUID: {}", e)),
                }
            }
        }
    }

    async fn collection_metadata(&self, collection_id: &str) -> Result<Option<Collection>> {
        // First get UUID
        let uuid = match self.get_uuid(collection_id).await? {
            Some(uuid) => uuid,
            None => return Ok(None),
        };

        let db = self.get_db().await?;

        match db {
            DbHandle::Transactional(db) => {
                let cf = self.get_cf(&db, CF_COLLECTIONS)?;
                let result = db.get_cf(&cf, uuid.as_bytes());
                drop(cf); // Release CF reference before await
                match result {
                    Ok(Some(data)) => {
                        let len = data.len() as u64;
                        self.update_stats(StatOp::Read(len)).await;
                        Ok(Some(Self::deserialize_record(&data)?))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(anyhow::anyhow!("Failed to get collection: {}", e)),
                }
            }
            DbHandle::Regular(db) => {
                let cf = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                let result = db.get_cf(&cf, uuid.as_bytes());
                match result {
                    Ok(Some(data)) => {
                        let len = data.len() as u64;
                        self.update_stats(StatOp::Read(len)).await;
                        Ok(Some(Self::deserialize_record(&data)?))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(anyhow::anyhow!("Failed to get collection: {}", e)),
                }
            }
        }
    }

    async fn get_collection(&self, collection_id: &str) -> Result<Option<Collection>> {
        // collection_metadata already returns a Collection
        self.collection_metadata(collection_id).await
    }

    async fn list_collections(&self) -> Result<Vec<Collection>> {
        let db = self.get_db().await?;
        let mut collections = Vec::new();

        match db {
            DbHandle::Transactional(db) => {
                let cf = self.get_cf(&db, CF_COLLECTIONS)?;
                let iter = db.iterator_cf(&cf, IteratorMode::Start);

                let mut total_bytes = 0u64;
                for item in iter {
                    let (_, value) = item?;
                    total_bytes += value.len() as u64;
                    collections.push(Self::deserialize_record(&value)?);
                }
                drop(cf); // Release CF reference before await
                self.update_stats(StatOp::Read(total_bytes)).await;
            }
            DbHandle::Regular(db) => {
                let cf = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                let iter = db.iterator_cf(&cf, IteratorMode::Start);

                for item in iter {
                    let (_, value) = item?;
                    self.update_stats(StatOp::Read(value.len() as u64)).await;
                    collections.push(Self::deserialize_record(&value)?);
                }
            }
        }

        Ok(collections)
    }

    async fn collection_exists(&self, collection_id: &str) -> Result<bool> {
        Ok(self.get_uuid(collection_id).await?.is_some())
    }

    async fn collection_id_exists(&self, collection_id: &str) -> Result<bool> {
        // Fast check using RocksDB key existence without fetching value
        let db = self.get_db().await?;

        match db {
            DbHandle::Transactional(db) => {
                let cf = self.get_cf(&db, CF_COLLECTIONS)?;
                // TransactionDB doesn't have key_may_exist_cf, use get_cf instead
                Ok(db.get_cf(&cf, collection_id.as_bytes())?.is_some())
            }
            DbHandle::Regular(db) => {
                let cf = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                Ok(db.key_may_exist_cf(&cf, collection_id.as_bytes()))
            }
        }
    }
}

impl RocksDbMetadataBackend {
    /// Create or update collection record
    pub async fn upsert_collection_record(&self, record: Collection) -> Result<()> {
        let db = self.get_db().await?;
        let data = Self::serialize_record(&record)?;
        let data_len = data.len() as u64;

        match db {
            DbHandle::Transactional(db) => {
                let txn = db.transaction();

                // Write to collections CF
                let cf_collections = self.get_cf(&db, CF_COLLECTIONS)?;
                txn.put_cf(&cf_collections, record.id.as_bytes(), &data)?;

                // Update name index
                let cf_name_index = self.get_cf(&db, CF_NAME_INDEX)?;
                let name = record.config.as_ref().map(|c| &c.name);
                txn.put_cf(&cf_name_index, name.as_bytes(), record.id.as_bytes())?;

                // Update UUID index (reverse lookup)
                let cf_uuid_index = self.get_cf(&db, CF_UUID_INDEX)?;
                txn.put_cf(&cf_uuid_index, record.id.as_bytes(), name.as_bytes())?;

                // Commit transaction
                txn.commit()?;
                self.update_stats(StatOp::Write(data_len)).await;

                let name = record
                    .config
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| record.id.clone());
                info!("✅ Upserted collection {} ({})", name, record.id);
            }
            DbHandle::Regular(db) => {
                let mut batch = WriteBatch::default();

                // Write to collections CF
                let cf_collections = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                batch.put_cf(&cf_collections, record.id.as_bytes(), &data);

                // Update name index
                let cf_name_index = db
                    .cf_handle(CF_NAME_INDEX)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                let name = record.config.as_ref().map(|c| &c.name);
                batch.put_cf(&cf_name_index, name.as_bytes(), record.id.as_bytes());

                // Update UUID index
                let cf_uuid_index = db
                    .cf_handle(CF_UUID_INDEX)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                batch.put_cf(&cf_uuid_index, record.id.as_bytes(), name.as_bytes());

                // Write batch atomically
                db.write(batch)?;
                self.update_stats(StatOp::Write(data_len)).await;

                let name = record
                    .config
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| record.id.clone());
                info!("✅ Upserted collection {} ({})", name, record.id);
            }
        }

        Ok(())
    }

    /// Create or update collection using protobuf format (unified API)
    pub async fn upsert_collection_proto(&self, proto_collection: &Collection) -> Result<()> {
        let db = self.get_db().await?;

        // Serialize to protobuf binary
        let mut data = Vec::new();
        proto_collection.encode(&mut data)?;
        let data_len = data.len() as u64;

        // Extract key fields from proto
        let uuid = &proto_collection.id;
        let config = proto_collection
            .config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection config is required"))?;
        let name = &config.name;

        match db {
            DbHandle::Transactional(db) => {
                let txn = db.transaction();

                // Write to collections CF with proto data
                let cf_collections = self.get_cf(&db, CF_COLLECTIONS)?;
                txn.put_cf(&cf_collections, uuid.as_bytes(), &data)?;

                // Update name index
                let cf_name_index = self.get_cf(&db, CF_NAME_INDEX)?;
                txn.put_cf(&cf_name_index, name.as_bytes(), uuid.as_bytes())?;

                // Update UUID index (reverse lookup)
                let cf_uuid_index = self.get_cf(&db, CF_UUID_INDEX)?;
                txn.put_cf(&cf_uuid_index, uuid.as_bytes(), name.as_bytes())?;

                // Tag indexes removed - metadata field no longer exists in Collection proto

                // Commit transaction
                txn.commit()?;
                self.update_stats(StatOp::Write(data_len)).await;

                info!(
                    "✅ Upserted collection {} ({}) using proto format",
                    name, uuid
                );
            }
            DbHandle::Regular(db) => {
                let mut batch = WriteBatch::default();

                // Write to collections CF with proto data
                let cf_collections = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                batch.put_cf(&cf_collections, uuid.as_bytes(), &data);

                // Update name index
                let cf_name_index = db
                    .cf_handle(CF_NAME_INDEX)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                batch.put_cf(&cf_name_index, name.as_bytes(), uuid.as_bytes());

                // Update UUID index
                let cf_uuid_index = db
                    .cf_handle(CF_UUID_INDEX)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                batch.put_cf(&cf_uuid_index, uuid.as_bytes(), name.as_bytes());

                // Tag indexes removed - metadata field no longer exists in Collection proto

                // Write batch atomically
                db.write(batch)?;
                self.update_stats(StatOp::Write(data_len)).await;

                info!(
                    "✅ Upserted collection {} ({}) using proto format",
                    name, uuid
                );
            }
        }

        Ok(())
    }

    /// Get collection as protobuf format
    pub async fn get_collection_proto(&self, collection_id: &str) -> Result<Option<Collection>> {
        let db = self.get_db().await?;

        // First try by UUID
        let data = match db {
            DbHandle::Transactional(ref db) => {
                let cf = self.get_cf(db, CF_COLLECTIONS)?;
                db.get_cf(&cf, collection_id.as_bytes())?
            }
            DbHandle::Regular(ref db) => {
                let cf = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                db.get_cf(&cf, collection_id.as_bytes())?
            }
        };

        if let Some(data) = data {
            // Try to decode as protobuf first
            match Collection::decode(&data[..]) {
                Ok(proto) => {
                    self.update_stats(StatOp::Read(data.len() as u64)).await;
                    return Ok(Some(proto));
                }
                Err(_) => {
                    // Fall back to trying Avro format (for backward compatibility)
                    debug!("Failed to decode as proto, trying Avro format");
                }
            }
        }

        // Try by name if UUID lookup failed
        let uuid = match self.get_uuid(collection_id).await? {
            Some(uuid) => uuid,
            None => return Ok(None),
        };

        // Fetch by UUID
        let data = match db {
            DbHandle::Transactional(ref db) => {
                let cf = self.get_cf(db, CF_COLLECTIONS)?;
                db.get_cf(&cf, uuid.as_bytes())?
            }
            DbHandle::Regular(ref db) => {
                let cf = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                db.get_cf(&cf, uuid.as_bytes())?
            }
        };

        match data {
            Some(data) => {
                // Try to decode as protobuf
                match Collection::decode(&data[..]) {
                    Ok(proto) => {
                        self.update_stats(StatOp::Read(data.len() as u64)).await;
                        Ok(Some(proto))
                    }
                    Err(e) => {
                        warn!("Failed to decode collection as proto: {}", e);
                        Ok(None)
                    }
                }
            }
            None => Ok(None),
        }
    }

    /// List collections by tag
    pub async fn list_collections_by_tag(&self, tag: &str) -> Result<Vec<Collection>> {
        let db = self.get_db().await?;
        let mut collections = Vec::new();

        match db {
            DbHandle::Transactional(ref db) => {
                let cf_tags = self.get_cf(db, CF_TAGS)?;
                let prefix = format!("{}:", tag);
                let iter = db.prefix_iterator_cf(&cf_tags, prefix.as_bytes());

                let mut seen_uuids = std::collections::HashSet::new();
                for item in iter {
                    let (key, uuid_bytes) = item?;
                    let uuid = String::from_utf8_lossy(&uuid_bytes).to_string();

                    if seen_uuids.insert(uuid.clone()) {
                        // Get the full collection record
                        let cf_collections = self.get_cf(db, CF_COLLECTIONS)?;
                        if let Ok(Some(data)) = db.get_cf(&cf_collections, uuid.as_bytes()) {
                            // Try to decode as proto first, then fall back to bincode
                            if let Ok(proto) = Collection::decode(&data[..]) {
                                collections.push(proto);
                            } else if let Ok(record) = Self::deserialize_record(&data) {
                                collections.push(record);
                            }
                        }
                    }
                }
            }
            DbHandle::Regular(ref db) => {
                let cf_tags = db
                    .cf_handle(CF_TAGS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                let prefix = format!("{}:", tag);
                let iter = db.prefix_iterator_cf(&cf_tags, prefix.as_bytes());

                let mut seen_uuids = std::collections::HashSet::new();
                for item in iter {
                    let (key, uuid_bytes) = item?;
                    let uuid = String::from_utf8_lossy(&uuid_bytes).to_string();

                    if seen_uuids.insert(uuid.clone()) {
                        // Get the full collection record
                        let cf_collections = db
                            .cf_handle(CF_COLLECTIONS)
                            .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                        if let Ok(Some(data)) = db.get_cf(&cf_collections, uuid.as_bytes()) {
                            // Try to decode as proto first, then fall back to bincode
                            if let Ok(proto) = Collection::decode(&data[..]) {
                                collections.push(proto);
                            } else if let Ok(record) = Self::deserialize_record(&data) {
                                collections.push(record);
                            }
                        }
                    }
                }
            }
        }

        Ok(collections)
    }

    /// Create a backup of the RocksDB database
    pub async fn create_backup(&self) -> Result<()> {
        if let Some(ref backup_config) = self.config.backup_config {
            let backup_path = &backup_config.backup_path;

            // Create backup directory if it doesn't exist
            tokio::fs::create_dir_all(backup_path)
                .await
                .context("Failed to create backup directory")?;

            // Get DB handle
            let db = self.get_db().await?;

            match db {
                DbHandle::Transactional(_db) => {
                    // TransactionDB doesn't support direct checkpoints
                    // Use backup engine instead
                    return Err(anyhow::anyhow!(
                        "TransactionDB backup not implemented - use backup engine API"
                    ));
                }
                DbHandle::Regular(ref db) => {
                    // Create checkpoint for regular DB
                    let checkpoint = rocksdb::checkpoint::Checkpoint::new(db)?;
                    let backup_dir =
                        backup_path.join(format!("backup_{}", chrono::Utc::now().timestamp()));
                    checkpoint.create_checkpoint(&backup_dir)?;
                    info!("✅ Created RocksDB backup at: {:?}", backup_dir);
                }
            }

            // Clean up old backups if needed
            if backup_config.max_backups > 0 {
                self.cleanup_old_backups(backup_path, backup_config.max_backups)
                    .await?;
            }

            Ok(())
        } else {
            Err(anyhow::anyhow!("Backup configuration not provided"))
        }
    }

    /// Clean up old backups keeping only the most recent N
    async fn cleanup_old_backups(&self, backup_path: &Path, max_backups: u32) -> Result<()> {
        let mut entries = tokio::fs::read_dir(backup_path).await?;
        let mut backups = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            if entry.file_name().to_string_lossy().starts_with("backup_") {
                backups.push(entry.path());
            }
        }

        // Sort by modification time (newest first)
        backups.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()));
        backups.reverse();

        // Remove old backups
        if backups.len() > max_backups as usize {
            for backup in &backups[max_backups as usize..] {
                tokio::fs::remove_dir_all(backup)
                    .await
                    .context("Failed to remove old backup")?;
                info!("🗑️ Removed old backup: {:?}", backup);
            }
        }

        Ok(())
    }

    /// Health check for RocksDB backend
    pub async fn health_check(&self) -> Result<()> {
        // Check if we can get DB handle
        let db = self.get_db().await?;

        // Try a simple read operation
        match db {
            DbHandle::Transactional(ref db) => {
                let cf = self.get_cf(db, CF_COLLECTIONS)?;
                let _ = db.get_cf(&cf, b"health_check")?;
            }
            DbHandle::Regular(ref db) => {
                let cf = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                let _ = db.get_cf(&cf, b"health_check")?;
            }
        }

        // Check statistics if enabled
        if self.config.enable_statistics {
            let stats = self.stats.read().await;
            debug!(
                "RocksDB Stats - Reads: {}, Writes: {}, Bytes read: {}, Bytes written: {}",
                stats.total_reads, stats.total_writes, stats.bytes_read, stats.bytes_written
            );
        }

        Ok(())
    }

    /// Delete collection by UUID
    pub async fn delete_collection_by_uuid(&self, uuid: &str) -> Result<bool> {
        // First get the collection to find its name
        let db = self.get_db().await?;

        let record = match db {
            DbHandle::Transactional(ref db) => {
                let cf = self.get_cf(db, CF_COLLECTIONS)?;
                match db.get_cf(&cf, uuid.as_bytes()) {
                    Ok(Some(data)) => Some(Self::deserialize_record(&data)?),
                    _ => None,
                }
            }
            DbHandle::Regular(ref db) => {
                let cf = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                match db.get_cf(&cf, uuid.as_bytes()) {
                    Ok(Some(data)) => Some(Self::deserialize_record(&data)?),
                    _ => None,
                }
            }
        };

        if let Some(record) = record {
            match db {
                DbHandle::Transactional(db) => {
                    let txn = db.transaction();

                    // Delete from collections CF
                    let cf_collections = self.get_cf(&db, CF_COLLECTIONS)?;
                    txn.delete_cf(&cf_collections, uuid.as_bytes())?;

                    // Delete from name index
                    let cf_name_index = self.get_cf(&db, CF_NAME_INDEX)?;
                    let name = record
                        .config
                        .as_ref()
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| record.id.clone());
                    txn.delete_cf(&cf_name_index, name.as_bytes())?;

                    // Delete from UUID index
                    let cf_uuid_index = self.get_cf(&db, CF_UUID_INDEX)?;
                    txn.delete_cf(&cf_uuid_index, uuid.as_bytes())?;

                    txn.commit()?;
                    self.update_stats(StatOp::Delete).await;

                    info!("🗑️ Deleted collection {} ({})", name, uuid);
                    Ok(true)
                }
                DbHandle::Regular(db) => {
                    let mut batch = WriteBatch::default();

                    // Delete from collections CF
                    let cf_collections = db
                        .cf_handle(CF_COLLECTIONS)
                        .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                    batch.delete_cf(&cf_collections, uuid.as_bytes());

                    // Delete from name index
                    let cf_name_index = db
                        .cf_handle(CF_NAME_INDEX)
                        .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                    let name = record
                        .config
                        .as_ref()
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| record.id.clone());
                    batch.delete_cf(&cf_name_index, name.as_bytes());

                    // Delete from UUID index
                    let cf_uuid_index = db
                        .cf_handle(CF_UUID_INDEX)
                        .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                    batch.delete_cf(&cf_uuid_index, uuid.as_bytes());

                    db.write(batch)?;
                    self.update_stats(StatOp::Delete).await;

                    info!("🗑️ Deleted collection {} ({})", name, uuid);
                    Ok(true)
                }
            }
        } else {
            Ok(false)
        }
    }

    /// Get collections by tag
    pub async fn get_collections_by_tag(&self, tag: &str) -> Result<Vec<Collection>> {
        let db = self.get_db().await?;
        let mut collections = Vec::new();

        match db {
            DbHandle::Transactional(db) => {
                let cf_tags = self.get_cf(&db, CF_TAGS)?;
                let cf_collections = self.get_cf(&db, CF_COLLECTIONS)?;

                // Scan tag index with prefix
                let prefix = format!("{}:", tag);
                let iter = db.prefix_iterator_cf(&cf_tags, prefix.as_bytes());

                for item in iter {
                    let (key, uuid_bytes) = item?;

                    // Check if key still starts with our prefix
                    if !key.starts_with(prefix.as_bytes()) {
                        break;
                    }

                    // Get collection data
                    if let Ok(Some(data)) = db.get_cf(&cf_collections, &uuid_bytes) {
                        collections.push(Self::deserialize_record(&data)?);
                    }
                }
            }
            DbHandle::Regular(db) => {
                let cf_tags = db
                    .cf_handle(CF_TAGS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;
                let cf_collections = db
                    .cf_handle(CF_COLLECTIONS)
                    .ok_or_else(|| anyhow::anyhow!("Column family not found"))?;

                // Scan tag index with prefix
                let prefix = format!("{}:", tag);
                let iter = db.prefix_iterator_cf(&cf_tags, prefix.as_bytes());

                for item in iter {
                    let (key, uuid_bytes) = item?;

                    // Check if key still starts with our prefix
                    if !key.starts_with(prefix.as_bytes()) {
                        break;
                    }

                    // Get collection data
                    if let Ok(Some(data)) = db.get_cf(&cf_collections, &uuid_bytes) {
                        collections.push(Self::deserialize_record(&data)?);
                    }
                }
            }
        }

        Ok(collections)
    }

    /// Get backend statistics
    pub async fn get_statistics(&self) -> BackendStatistics {
        self.stats.read().await.clone()
    }
}

impl Clone for BackendStatistics {
    fn clone(&self) -> Self {
        Self {
            total_reads: self.total_reads,
            total_writes: self.total_writes,
            total_deletes: self.total_deletes,
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
            bytes_written: self.bytes_written,
            bytes_read: self.bytes_read,
            compactions: self.compactions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_rocksdb_backend_basic_operations() {
        let temp_dir = TempDir::new().unwrap();
        let config = RocksDbMetadataConfig {
            db_path: temp_dir.path().join("rocksdb"),
            ..Default::default()
        };

        let backend = RocksDbMetadataBackend::new(config).await.unwrap();

        // Test upsert
        let record = Collection {
            uuid: "test-uuid-123".to_string(),
            name: "test_collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            storage_engine: "viper".to_string(),
            timestamp: 1000,
            updated_at: Some(1000),
            version: Some(1),
            vector_count: 0,
            total_size_bytes: 0,
            config: "{}".to_string(),
            description: Some("Test collection".to_string()),
            tags: vec!["test".to_string(), "rocksdb".to_string()],
            owner: Some("test_user".to_string()),
        };

        backend
            .upsert_collection_record(record.clone())
            .await
            .unwrap();

        // Test get by name
        let retrieved = backend
            .collection_metadata("test_collection")
            .await
            .unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.uuid, "test-uuid-123");
        assert_eq!(retrieved.name, "test_collection");

        // Test get by UUID
        let uuid = backend.get_uuid("test_collection").await.unwrap();
        assert_eq!(uuid, Some("test-uuid-123".to_string()));

        // Test list all
        let all = backend.list_collections().await.unwrap();
        assert_eq!(all.len(), 1);

        // Test get by tag
        let tagged = backend.get_collections_by_tag("test").await.unwrap();
        assert_eq!(tagged.len(), 1);

        // Test delete
        let deleted = backend
            .delete_collection_by_uuid("test-uuid-123")
            .await
            .unwrap();
        assert!(deleted);

        // Verify deletion
        let after_delete = backend
            .collection_metadata("test_collection")
            .await
            .unwrap();
        assert!(after_delete.is_none());
    }

    #[tokio::test]
    async fn test_rocksdb_backend_transactions() {
        let temp_dir = TempDir::new().unwrap();
        let config = RocksDbMetadataConfig {
            db_path: temp_dir.path().join("rocksdb_txn"),
            enable_transactions: true,
            ..Default::default()
        };

        let backend = RocksDbMetadataBackend::new(config).await.unwrap();

        // Create multiple collections
        for i in 0..5 {
            let record = Collection {
                uuid: format!("uuid-{}", i),
                name: format!("collection_{}", i),
                dimension: 128,
                distance_metric: "euclidean".to_string(),
                indexing_algorithm: "flat".to_string(),
                storage_engine: "lsm".to_string(),
                timestamp: 1000 + i,
                updated_at: 1000 + i,
                version: Some(1),
                vector_count: i * 100,
                total_size_bytes: i * 1024,
                config: "{}".to_string(),
                description: None,
                tags: vec!["batch".to_string()],
                owner: None,
            };

            backend.upsert_collection_record(record).await.unwrap();
        }

        // Verify all were created
        let all = backend.list_collections().await.unwrap();
        assert_eq!(all.len(), 5);

        // Test batch operations
        let tagged = backend.get_collections_by_tag("batch").await.unwrap();
        assert_eq!(tagged.len(), 5);
    }
}
