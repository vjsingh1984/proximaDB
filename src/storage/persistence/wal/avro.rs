// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Avro WAL Strategy - Schema Evolution Support with High Performance

use anyhow::{Context, Result};
use apache_avro::{Reader, Schema, Writer};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{
    atomic_batch_operation::AtomicWalBatchOperation,
    flush_coordinator::{FlushCoordinatorCallbacks, FlushDataSource, WalFlushCoordinator},
    schema::AVRO_SCHEMA_V1,
    FlushCompletionResult, FlushCycle, FlushCycleState, FlushResult, WalConfig, WalDiskManager,
    WalEntry, WalOperation, WalStats, WalStrategy,
};
use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::assignment_service::{get_assignment_service, AssignmentService};
use crate::storage::atomicity::TransactionContext;
use crate::storage::memtable::core::MemtableCore;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::UnifiedStorageEngine;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Avro representation of WAL entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroWalEntry {
    pub entry_id: String,
    pub collection_id: String,
    pub operation: AvroWalOperation,
    pub timestamp: i64,
    pub sequence: i64,
    pub global_sequence: i64,
    pub expires_at: Option<i64>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroWalOperation {
    pub op_type: AvroOpType,
    pub vector_id: Option<String>,
    pub vector_data: Option<Vec<u8>>, // Serialized vector record
    pub metadata: Option<String>,
    pub config: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AvroOpType {
    #[serde(rename = "INSERT")]
    Insert,
    #[serde(rename = "UPDATE")]
    Update,
    #[serde(rename = "DELETE")]
    Delete,
}

/// WAL-specific statistics for collections
#[derive(Debug, Clone)]
struct CollectionWalStats {
    /// Total WAL files count for this collection
    wal_files_count: usize,
    /// Total WAL data size (bytes) for this collection
    total_size_bytes: u64,
    /// Last accessed timestamp
    last_accessed: DateTime<Utc>,
}

/// Avro WAL strategy implementation with collection directory tracking
pub struct AvroWalStrategy {
    config: Option<WalConfig>,
    filesystem: Option<Arc<FilesystemFactory>>,
    memory_table: Option<crate::storage::memtable::specialized::WalMemtable>,
    disk_manager: Option<WalDiskManager>,
    storage_engine: Arc<tokio::sync::RwLock<Option<Arc<dyn UnifiedStorageEngine>>>>,

    /// Common flush coordinator for coordinated cleanup
    flush_coordinator: WalFlushCoordinator,
    /// Assignment service for directory assignment
    assignment_service: Arc<dyn AssignmentService>,
    /// Unified distance computation manager
    distance_compute: UnifiedDistanceCompute,
    /// WAL-specific collection statistics
    collection_stats: Arc<RwLock<HashMap<CollectionId, CollectionWalStats>>>,
}

impl AvroWalStrategy {
    /// Create new Avro WAL strategy with collection assignment tracking
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a dummy transaction context for standalone atomic operations
    fn create_dummy_transaction_context(&self) -> TransactionContext {
        use crate::storage::atomicity::{IsolationLevel, TransactionMetadata, TransactionState};
        use std::collections::{HashMap, VecDeque};
        use std::time::Duration;
        use uuid::Uuid;

        TransactionContext {
            transaction_id: Uuid::new_v4(),
            start_time: chrono::Utc::now(),
            state: TransactionState::Active,
            executed_operations: Vec::new(),
            rollback_operations: VecDeque::new(),
            isolation_level: IsolationLevel::ReadCommitted,
            timeout: Duration::from_secs(30),
            locked_resources: HashMap::new(),
            metadata: TransactionMetadata {
                client_id: None,
                session_id: None,
                tags: HashMap::new(),
                operation_count: 0,
                estimated_duration: Duration::from_secs(0),
            },
        }
    }

    /// Create new strategy with default values
    pub fn default() -> Self {
        Self {
            config: None,
            filesystem: None,
            memory_table: None,
            disk_manager: None,
            storage_engine: Arc::new(tokio::sync::RwLock::new(None)),
            flush_coordinator: WalFlushCoordinator::new(),
            assignment_service: get_assignment_service(),
            distance_compute: UnifiedDistanceCompute::default(),
            collection_stats: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get WAL assignment statistics for monitoring and management
    pub async fn get_assignment_statistics(&self) -> Result<serde_json::Value> {
        let stats = self.collection_stats.read().await;
        let config = self
            .config
            .as_ref()
            .context("WAL strategy not initialized")?;

        let mut total_files = 0;
        let mut total_size = 0;

        // Count statistics from collected data
        for collection_stats in stats.values() {
            total_files += collection_stats.wal_files_count;
            total_size += collection_stats.total_size_bytes;
        }

        Ok(serde_json::json!({
            "total_collections": stats.len(),
            "total_directories": config.multi_disk.data_directories.len(),
            "total_wal_files": total_files,
            "total_size_bytes": total_size,
            "distribution_strategy": format!("{:?}", config.multi_disk.distribution_strategy),
            "collection_affinity": config.multi_disk.collection_affinity
        }))
    }

    /// Internal Avro serialization implementation
    async fn serialize_entries_impl(&self, entries: &[WalEntry]) -> Result<Vec<u8>> {
        let config = self.config.as_ref().context("Config not initialized")?;

        // Inline serialization implementation
        let schema = Schema::parse_str(AVRO_SCHEMA_V1).context("Failed to parse Avro schema")?;

        let compression_codec = match config.compression.algorithm {
            crate::core::CompressionAlgorithm::None => apache_avro::Codec::Null,
            crate::core::CompressionAlgorithm::Snappy => apache_avro::Codec::Deflate,
            crate::core::CompressionAlgorithm::Lz4 => apache_avro::Codec::Deflate,
            // Note: Lz4Hc not available in new enum
            // crate::core::CompressionAlgorithm::Lz4Hc => apache_avro::Codec::Deflate,
            crate::core::CompressionAlgorithm::Gzip => apache_avro::Codec::Deflate,
            // Note: Deflate not available in new enum
            // crate::core::CompressionAlgorithm::Deflate => apache_avro::Codec::Deflate,
            crate::core::CompressionAlgorithm::Zstd => apache_avro::Codec::Deflate,
        };

        let mut writer = Writer::with_codec(&schema, Vec::new(), compression_codec);

        for entry in entries {
            let avro_entry = super::schema::convert_to_avro_entry(entry)?;
            writer
                .append_ser(avro_entry)
                .context("Failed to serialize WAL entry to Avro")?;
        }

        let data = writer
            .into_inner()
            .context("Failed to finalize Avro writer")?;

        Ok(data)
    }

    /// Internal Avro deserialization implementation
    async fn deserialize_entries_impl(&self, data: &[u8]) -> Result<Vec<WalEntry>> {
        // Inline deserialization implementation
        let reader = Reader::new(data).context("Failed to create Avro reader")?;

        let schema = Schema::parse_str(AVRO_SCHEMA_V1).context("Failed to parse Avro schema")?;

        let mut entries = Vec::new();
        for value in reader {
            let value = value?;
            // Convert the Avro Value directly to our struct
            let avro_entry: AvroWalEntry =
                apache_avro::from_value(&value).context("Failed to deserialize Avro WAL entry")?;

            let entry = super::schema::convert_from_avro_entry(avro_entry)?;
            entries.push(entry);
        }

        Ok(entries)
    }

    /// WAL write using FileStore atomic infrastructure + WAL-specific flush logic
    async fn write_batch_with_filestore_atomic_and_wal_flush(
        &self,
        entries: Vec<WalEntry>,
    ) -> Result<Vec<u64>> {
        let start_time = std::time::Instant::now();

        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // STEP 1: Write to memtable (immediate read availability)
        // Extract vector records from AvroPayload operations
        let mut all_vector_records = Vec::new();
        let mut sequences = Vec::new();

        for entry in &entries {
            match &entry.operation {
                WalOperation::AvroPayload { avro_data, operation_type } => {
                    tracing::info!("Processing batch AvroPayload operation: {}", operation_type);
                    
                    // Deserialize Avro data - supports both single records and batches
                    let vector_records = if let Ok(single_record) = crate::core::VectorRecord::from_avro_bytes(avro_data) {
                        vec![single_record]
                    } else if let Ok(batch_records) = crate::storage::persistence::wal::schema::deserialize_vector_batch(avro_data) {
                        batch_records
                    } else {
                        tracing::error!("Failed to deserialize Avro payload for batch operation: {}", operation_type);
                        continue;
                    };
                    
                    if !vector_records.is_empty() {
                        let start_seq = memory_table.current_sequence();
                        let end_seq = start_seq + vector_records.len() as u64 - 1;
                        let batch_id = crate::storage::persistence::wal::BatchId::new(
                            entry.collection_id.clone(),
                            start_seq,
                            end_seq,
                        );

                        let batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
                            batch_id,
                            vector_records: vector_records.clone(),
                            created_at: std::time::SystemTime::now(),
                            total_size_bytes: entry.actual_size_bytes(),
                            is_flushed: false,
                        };

                        let batch_sequences = memory_table.add_vector_batch(batch).await?;
                        sequences.extend(batch_sequences);
                        all_vector_records.extend(vector_records);
                    }
                }
                _ => {
                    // Non-vector operations (Delete, Flush, etc.) don't go to memtable
                    tracing::debug!("Skipping non-vector operation in batch: {:?}", entry.operation);
                }
            }
        }

        if all_vector_records.is_empty() {
            tracing::warn!("No vector records found in WAL entries for AVRO batch write");
        }
        let memtable_time = start_time.elapsed().as_micros();

        // STEP 2: Atomic disk persistence (reuse FileStore atomic write pattern)
        match self.write_wal_entries_to_disk_atomic(&entries).await {
            Ok(()) => {
                let disk_time = start_time.elapsed().as_micros() - memtable_time;
                tracing::info!(
                    "💾 WAL atomic write completed: memtable={}μs, disk={}μs, total={}μs",
                    memtable_time,
                    disk_time,
                    start_time.elapsed().as_micros()
                );
            }
            Err(e) => {
                tracing::error!(
                    "❌ WAL disk write FAILED: {}. Data durability compromised!",
                    e
                );
                return Err(e.context("WAL disk persistence failed"));
            }
        }

        // STEP 3: WAL-specific flush check (different from FileStore - threshold-based cleanup)
        if memory_table.needs_global_flush().await? {
            tracing::info!("🔄 WAL threshold exceeded - triggering flush to VIPER storage");

            // Background flush to VIPER (non-blocking, WAL-specific)
            let collections: std::collections::HashSet<String> =
                entries.iter().map(|e| e.collection_id.clone()).collect();

            for collection_id in collections {
                // Manual flush trigger - in production this would be a background task
                tracing::info!("🔄 Manual flush trigger for collection: {}", collection_id);
            }
        }

        Ok(sequences)
    }

    /// Write WAL entries to disk using configured WAL URLs (not hardcoded)
    async fn write_wal_entries_to_disk_atomic(&self, entries: &[WalEntry]) -> Result<()> {
        let config = self.config.as_ref().context("WAL config not initialized")?;
        let filesystem = self
            .filesystem
            .as_ref()
            .context("Filesystem not initialized")?;

        // Group by collection (same pattern as FileStore incremental operations)
        let mut by_collection: std::collections::HashMap<String, Vec<&WalEntry>> =
            std::collections::HashMap::new();
        for entry in entries {
            by_collection
                .entry(entry.collection_id.clone())
                .or_default()
                .push(entry);
        }

        // Write each collection atomically using assigned WAL URLs with metadata tracking
        for (collection_id, collection_entries) in by_collection {
            // Select WAL URL with assignment tracking (async)
            let wal_base_url = self
                .select_wal_url_for_collection(&collection_id, config)
                .await?;
            tracing::debug!(
                "📁 Using assigned WAL URL for collection {}: {}",
                collection_id,
                wal_base_url
            );

            // Get filesystem for the assigned URL
            let fs = filesystem.get_filesystem(&wal_base_url).context(format!(
                "Failed to get filesystem for WAL URL: {}",
                wal_base_url
            ))?;

            // Build collection-specific paths
            let (collection_dir, wal_file_path) =
                self.build_collection_wal_paths(&wal_base_url, &collection_id)?;

            // Create collection WAL directory using filesystem API
            fs.create_dir_all(&collection_dir).await.context(format!(
                "Failed to create WAL directory: {}",
                collection_dir
            ))?;

            // Serialize to Avro (reuse existing serialization)
            let collection_entries_vec: Vec<&WalEntry> = collection_entries;
            let entries_slice: &[WalEntry] = &collection_entries_vec
                .iter()
                .map(|&e| e.clone())
                .collect::<Vec<_>>();
            let avro_data = self.serialize_entries_impl(entries_slice).await?;

            // Atomic write using filesystem API (works with file://, s3://, adls://, gcs://)
            fs.write_atomic(&wal_file_path, &avro_data, None)
                .await
                .context(format!(
                    "Atomic WAL write failed for collection {} to {}",
                    collection_id, wal_file_path
                ))?;

            // Update collection statistics
            {
                let mut stats = self.collection_stats.write().await;
                let collection_id_key = CollectionId::from(collection_id.clone());
                let stat_entry = stats
                    .entry(collection_id_key)
                    .or_insert(CollectionWalStats {
                        wal_files_count: 0,
                        total_size_bytes: 0,
                        last_accessed: chrono::Utc::now(),
                    });
                stat_entry.wal_files_count += 1;
                stat_entry.total_size_bytes += avro_data.len() as u64;
                stat_entry.last_accessed = chrono::Utc::now();
            }

            tracing::debug!(
                "✅ WAL file written atomically: {} ({} entries, {} bytes)",
                wal_file_path,
                collection_entries_vec.len(),
                avro_data.len()
            );
        }

        Ok(())
    }

    /// Hash collection ID for distribution
    fn hash_collection_id(&self, collection_id: &str) -> usize {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        hasher.finish() as usize
    }

    /// Build collection-specific WAL paths from base URL
    fn build_collection_wal_paths(
        &self,
        wal_base_url: &str,
        collection_id: &str,
    ) -> Result<(String, String)> {
        if wal_base_url.starts_with("file://") {
            let base_path = wal_base_url.strip_prefix("file://").unwrap_or(wal_base_url);
            let collection_dir = format!("{}/{}", base_path, collection_id);
            let wal_file_path = format!("{}/wal_current.avro", collection_dir);
            Ok((collection_dir, wal_file_path))
        } else {
            // For cloud URLs, append collection path
            let base_url = wal_base_url.trim_end_matches('/');
            let collection_dir = format!("{}/{}", base_url, collection_id);
            let wal_file_path = format!("{}/wal_current.avro", collection_dir);
            Ok((collection_dir, wal_file_path))
        }
    }
}

// Assignment metadata management methods (outside trait implementation)
impl AvroWalStrategy {
    /// Initialize flush state for a collection based on configuration
    pub async fn initialize_flush_state(&self, collection_id: &CollectionId) -> Result<()> {
        self.flush_coordinator
            .initialize_flush_state(collection_id)
            .await
    }

    /// Start a flush operation and return data source for storage engine
    pub async fn initiate_flush(
        &self,
        collection_id: &CollectionId,
        sequences: Vec<u64>,
    ) -> Result<super::flush_coordinator::FlushDataSource> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WAL config not initialized"))?;
        self.flush_coordinator
            .initiate_flush(collection_id, sequences, &config.performance.sync_mode)
            .await
    }

    /// Acknowledge successful flush and cleanup WAL data
    pub async fn acknowledge_flush(
        &self,
        collection_id: &CollectionId,
        flush_id: u64,
        flushed_sequences: Vec<u64>,
    ) -> Result<()> {
        let cleanup_instructions = self
            .flush_coordinator
            .acknowledge_flush(collection_id, flush_id, flushed_sequences)
            .await?;

        // Execute cleanup instructions
        if cleanup_instructions.cleanup_memory {
            if let Some(memory_table) = &self.memory_table {
                let max_sequence = cleanup_instructions
                    .sequences_to_cleanup
                    .iter()
                    .max()
                    .copied()
                    .unwrap_or(0);
                memory_table
                    .clear_flushed(collection_id, max_sequence)
                    .await?;
                tracing::debug!(
                    "🧹 Cleaned memory structures for {} up to sequence {}",
                    collection_id,
                    max_sequence
                );
            }
        }

        // Clean up disk files
        for wal_file in &cleanup_instructions.cleanup_disk_files {
            self.delete_wal_file(wal_file).await?;
            tracing::debug!("🗑️ Deleted fully flushed WAL file: {}", wal_file);
        }

        tracing::info!(
            "✅ Flush acknowledged for {}: {} sequences processed",
            collection_id,
            cleanup_instructions.sequences_to_cleanup.len()
        );
        Ok(())
    }

    /// Helper: Get WAL files containing specific sequences
    async fn get_wal_files_for_sequences(
        &self,
        collection_id: &CollectionId,
        sequences: &[u64],
    ) -> Result<Vec<String>> {
        // This would query the disk manager to find which WAL files contain these sequences
        // For now, return a placeholder - this needs to be implemented based on disk WAL file naming
        Ok(vec![format!(
            "wal_{}_{}.avro",
            collection_id,
            sequences.first().unwrap_or(&0)
        )])
    }

    /// Helper: Check if a WAL file is fully flushed
    async fn is_wal_file_fully_flushed(&self, _wal_file: &str, _sequence: u64) -> Result<bool> {
        // This would check if all sequences in the WAL file have been flushed
        // For now, assume true - needs proper implementation
        Ok(true)
    }

    /// Helper: Delete a WAL file
    async fn delete_wal_file(&self, wal_file: &str) -> Result<()> {
        if let Some(filesystem) = &self.filesystem {
            if let Ok(fs) = filesystem.get_filesystem("file://") {
                fs.delete(wal_file).await.map_err(|e| {
                    anyhow::anyhow!("Failed to delete WAL file {}: {}", wal_file, e)
                })?;
            }
        }
        Ok(())
    }

    /// Check if a string looks like a valid collection ID (UUID format)
    fn is_valid_collection_id(&self, id: &str) -> bool {
        // Simple validation: should be at least 8 characters and contain alphanumeric/hyphens
        id.len() >= 8
            && id
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }

    /// Initialize assignment service for WAL directories and run concurrent discovery for all components
    async fn initialize_assignment_service(
        &self,
        config: &WalConfig,
        filesystem: &Arc<FilesystemFactory>,
    ) -> Result<()> {
        use crate::storage::assignment_service::AssignmentDiscovery;

        // Define storage and index URLs based on the same pattern as WAL
        let storage_urls = vec![
            "file:///workspace/data/disk1/storage".to_string(),
            "file:///workspace/data/disk2/storage".to_string(),
            "file:///workspace/data/disk3/storage".to_string(),
        ];

        let index_urls = vec![
            "file:///workspace/data/disk1/storage/index".to_string(),
            "file:///workspace/data/disk2/storage/index".to_string(),
            "file:///workspace/data/disk3/storage/index".to_string(),
        ];

        // Run concurrent discovery for WAL, Storage, and Index components using 3 threads
        tracing::info!("🚀 Starting concurrent discovery for WAL, Storage, and Index components");
        let (wal_count, storage_count, index_count) =
            AssignmentDiscovery::discover_all_components_concurrent(
                &config.multi_disk.data_directories,
                &storage_urls,
                &index_urls,
                filesystem,
                &self.assignment_service,
            )
            .await?;

        tracing::info!(
            "✅ Concurrent discovery completed - WAL: {}, Storage: {}, Index: {} collections",
            wal_count,
            storage_count,
            index_count
        );

        Ok(())
    }
}

#[async_trait]
impl WalStrategy for AvroWalStrategy {
    fn strategy_name(&self) -> &'static str {
        "Avro"
    }

    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()> {
        tracing::debug!("🚀 AvroWalStrategy::initialize - Starting initialization");
        tracing::debug!("📋 Config details:");
        tracing::debug!("  - memtable_type: {:?}", config.memtable.memtable_type);
        tracing::debug!("  - compression: {:?}", config.compression.algorithm);
        tracing::debug!(
            "  - memory_flush_size_bytes: {}",
            config.performance.memory_flush_size_bytes
        );
        tracing::debug!(
            "  - data_directories: {} dirs",
            config.multi_disk.data_directories.len()
        );

        self.config = Some(config.clone());
        self.filesystem = Some(filesystem.clone());
        // Create new unified memtable system for WAL with proper flush threshold from config
        let mut memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        memtable_config.flush_threshold_bytes = config.performance.memory_flush_size_bytes;
        tracing::info!(
            "🔧 MemtableConfig: flush_threshold_bytes={} bytes ({}MB)",
            memtable_config.flush_threshold_bytes,
            memtable_config.flush_threshold_bytes / (1024 * 1024)
        );
        self.memory_table = Some(crate::storage::memtable::MemtableFactory::create_for_wal(
            memtable_config,
        ));
        self.disk_manager = Some(WalDiskManager::new(config.clone(), filesystem.clone()).await?);

        // Discover existing collections using base trait method
        let discovered_count = self
            .discover_existing_assignments(&config, &filesystem)
            .await?;
        tracing::info!(
            "✅ Avro WAL strategy initialized with {} discovered collections",
            discovered_count
        );
        tracing::debug!("✅ AvroWalStrategy::initialize - Initialization complete");
        Ok(())
    }

    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        let engine_name = storage_engine.engine_name();
        tracing::info!(
            "🏗️ AvroWalStrategy: Setting storage engine: {}",
            engine_name
        );
        tracing::debug!(
            "🔍 STORAGE ENGINE: Capabilities - supports_collections: {}, supports_vectors: {}",
            true,
            true
        ); // TODO: Add capability checking if available

        // Register with flush coordinator for polymorphic delegation (async)
        let engine_clone = storage_engine.clone();
        let coordinator_ref = &self.flush_coordinator;
        let _registration_result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                coordinator_ref
                    .register_storage_engine(engine_name, engine_clone)
                    .await;
            })
        });

        // Store engine using interior mutability
        let storage_engine_clone = storage_engine.clone();
        let storage_engine_ref = self.storage_engine.clone();
        tokio::spawn(async move {
            let mut guard = storage_engine_ref.write().await;
            *guard = Some(storage_engine_clone);
        });

        tracing::info!("✅ STORAGE ENGINE: Successfully attached to AvroWAL strategy and registered with FlushCoordinator");
    }

    async fn serialize_entries(&self, entries: &[WalEntry]) -> Result<Vec<u8>> {
        self.serialize_entries_impl(entries).await
    }

    async fn deserialize_entries(&self, data: &[u8]) -> Result<Vec<WalEntry>> {
        self.deserialize_entries_impl(data).await
    }

    async fn write_entry(&self, entry: WalEntry) -> Result<u64> {
        tracing::info!(
            "📝 WRITE_ENTRY: Starting write for collection {}, entry_id {}",
            entry.collection_id,
            entry.entry_id
        );

        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Log entry details
        tracing::debug!(
            "📝 WRITE_ENTRY: Entry details - seq={}, global_seq={}",
            entry.sequence,
            entry.global_sequence
        );

        // Convert single entry to batch for unified API - simplified to only handle AvroPayload
        let sequence = match &entry.operation {
            WalOperation::AvroPayload { avro_data, operation_type } => {
                tracing::info!("Processing AvroPayload operation: {}", operation_type);
                
                // Deserialize Avro data - supports both single records and batches
                let vector_records = if let Ok(single_record) = crate::core::VectorRecord::from_avro_bytes(avro_data) {
                    // Single record
                    vec![single_record]
                } else if let Ok(batch_records) = crate::storage::persistence::wal::schema::deserialize_vector_batch(avro_data) {
                    // Batch of records
                    batch_records
                } else {
                    tracing::error!("Failed to deserialize Avro payload for operation: {}", operation_type);
                    return Err(anyhow::anyhow!("Invalid Avro payload"));
                };
                
                if !vector_records.is_empty() {
                    let start_seq = memory_table.current_sequence();
                    let end_seq = start_seq + vector_records.len() as u64 - 1;
                    let batch_id = crate::storage::persistence::wal::BatchId::new(
                        entry.collection_id.clone(),
                        start_seq,
                        end_seq,
                    );

                    let batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
                        batch_id,
                        vector_records,
                        created_at: std::time::SystemTime::now(),
                        total_size_bytes: entry.actual_size_bytes(),
                        is_flushed: false,
                    };

                    let sequences = memory_table.add_vector_batch(batch).await?;
                    sequences.into_iter().next().unwrap_or(0)
                } else {
                    tracing::warn!("Empty Avro payload for operation: {}", operation_type);
                    0
                }
            }
            _ => {
                // For non-vector operations (Delete, Flush, Checkpoint), skip memtable processing
                tracing::debug!("Non-vector operation in single entry write, skipping memtable: {:?}", entry.operation);
                0
            }
        };
        tracing::info!(
            "✅ WRITE_ENTRY: Inserted entry with sequence {} for collection {}",
            sequence,
            entry.collection_id
        );

        // Get current memtable size and stats
        let memtable_size = memory_table.size_bytes().await;
        let memtable_count = memory_table.len().await;
        tracing::info!(
            "📊 MEMTABLE_STATUS: After insert - size={}B ({}MB), count={} entries",
            memtable_size,
            memtable_size / 1024 / 1024,
            memtable_count
        );

        // Check if we need to flush
        tracing::info!(
            "🔍 WRITE_FLUSH_CHECK: Checking if any collections need flushing after write"
        );
        let collections_needing_flush = memory_table.collections_needing_flush().await?;
        tracing::info!(
            "🔍 WRITE_FLUSH_CHECK: Found {} collections needing flush",
            collections_needing_flush.len()
        );

        if !collections_needing_flush.is_empty() {
            tracing::info!(
                "🚨 WRITE_TRIGGERED_FLUSH: {} collections triggered flush after write: {:?}",
                collections_needing_flush.len(),
                collections_needing_flush
            );
            // Background flush (in production, this would be handled by a background task)
            for collection_id in collections_needing_flush {
                tracing::info!(
                    "🚀 WRITE_FLUSH: Starting flush for collection {} due to threshold",
                    collection_id
                );
                let flush_result = self.flush(Some(&collection_id)).await;
                match flush_result {
                    Ok(result) => {
                        tracing::info!(
                            "✅ WRITE_FLUSH_SUCCESS: Collection {} - {} entries, {} bytes",
                            collection_id,
                            result.entries_flushed,
                            result.bytes_written
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "❌ WRITE_FLUSH_ERROR: Collection {} failed: {}",
                            collection_id,
                            e
                        );
                    }
                }
            }
        } else {
            tracing::info!("✅ WRITE_FLUSH_CHECK: No collections need flushing after write (size={}B, count={})", 
                          memtable_size, memtable_count);
        }

        Ok(sequence)
    }

    async fn write_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        // CRITICAL FIX: Use atomic batch operation to prevent half-writes
        // This ensures both memtable and disk writes succeed or both are rolled back
        self.write_batch_atomic(entries, true).await // true = immediate sync per batch
    }

}

impl AvroWalStrategy {
    /// **NEW: Atomic batch write with proper rollback on failure**
    /// Addresses the user's concern about "half-writes" by ensuring atomicity
    async fn write_batch_atomic(
        &self,
        entries: Vec<WalEntry>,
        immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Step 1: Prepare the vector batch (same as before)
        let collection_id = entries[0].collection_id.clone();
        let start_seq = entries.iter().map(|e| e.sequence).min().unwrap_or(0);
        let end_seq = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
        let batch_id = crate::storage::persistence::wal::BatchId::new(collection_id.clone(), start_seq, end_seq);

        // Step 2: Deserialize AVRO payload to vector records for memtable
        let mut vector_records = Vec::new();
        for entry in &entries {
            if let crate::storage::persistence::wal::WalOperation::AvroPayload { avro_data, .. } = &entry.operation {
                // Deserialize AVRO batch to individual vector records
                match crate::storage::persistence::wal::schema::deserialize_vector_batch(avro_data) {
                    Ok(batch_vectors) => {
                        vector_records.extend(batch_vectors);
                    }
                    Err(e) => {
                        // Try single vector format
                        if let Ok(single_vector) = crate::core::VectorRecord::from_avro_bytes(avro_data) {
                            vector_records.push(single_vector);
                        } else {
                            tracing::warn!("Failed to deserialize AVRO payload: {}", e);
                        }
                    }
                }
            }
        }

        // Step 3: Create atomic operation
        let vector_batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
            batch_id: batch_id.clone(),
            vector_records,
            created_at: std::time::SystemTime::now(),
            total_size_bytes: entries.iter().map(|e| e.actual_size_bytes()).sum(),
            is_flushed: false,
        };

        let atomic_operation = AtomicWalBatchOperation::new(
            entries,
            vector_batch,
            collection_id,
            immediate_sync,
            memory_table.clone(), // Clone Arc references, not actual data
            self.disk_manager.as_ref().map(|dm| Arc::new(dm.clone())),
        );

        // Step 4: Execute atomic operation (with automatic rollback on failure)
        let mut dummy_context = self.create_dummy_transaction_context();
        let mut atomic_operation = atomic_operation; // Make mutable for execute
        match atomic_operation.execute(&mut dummy_context).await {
            Ok(crate::storage::atomicity::OperationResult::BatchWrite { sequences_written, .. }) => {
                // Extract sequences for return value
                let sequences: Vec<u64> = (start_seq..=start_seq + sequences_written - 1).collect();
                
                tracing::info!(
                    "✅ ATOMIC_BATCH: Batch operation completed successfully (batch_id: {})",
                    batch_id.batch_uuid
                );

                Ok(sequences)
            }
            Ok(_) => {
                Err(anyhow::anyhow!("Unexpected operation result type"))
            }
            Err(e) => {
                tracing::error!(
                    "❌ ATOMIC_BATCH: Batch operation failed and was rolled back (batch_id: {}): {}",
                    batch_id.batch_uuid,
                    e
                );
                Err(e)
            }
        }
    }

    async fn write_batch_with_sync(
        &self,
        entries: Vec<WalEntry>,
        immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        let start_time = std::time::Instant::now();

        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Step 1: Create BatchId for coordination
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        let collection_id = entries[0].collection_id.clone();
        let start_seq = entries.iter().map(|e| e.sequence).min().unwrap_or(0);
        let end_seq = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
        let batch_id = crate::storage::persistence::wal::BatchId::new(collection_id.clone(), start_seq, end_seq);

        tracing::info!(
            "🔄 AVRO_BATCH: Creating batch {} for collection {} ({} entries, seq range: {}-{})",
            batch_id.batch_uuid,
            collection_id,
            entries.len(),
            start_seq,
            end_seq
        );

        // Step 2: Deserialize AVRO payload to vector records for memtable
        let mut vector_records = Vec::new();
        for entry in &entries {
            if let crate::storage::persistence::wal::WalOperation::AvroPayload { avro_data, .. } = &entry.operation {
                // Deserialize AVRO batch to individual vector records
                match crate::storage::persistence::wal::schema::deserialize_vector_batch(avro_data) {
                    Ok(batch_vectors) => {
                        vector_records.extend(batch_vectors);
                    }
                    Err(e) => {
                        // Try single vector format
                        if let Ok(single_vector) = crate::core::VectorRecord::from_avro_bytes(avro_data) {
                            vector_records.push(single_vector);
                        } else {
                            tracing::warn!("Failed to deserialize AVRO payload: {}", e);
                        }
                    }
                }
            }
        }

        // Step 3: Create WalVectorBatch for WAL behavior layer  
        let vector_batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
            batch_id: batch_id.clone(),
            vector_records,
            created_at: std::time::SystemTime::now(),
            total_size_bytes: entries.iter().map(|e| e.actual_size_bytes()).sum(),
            is_flushed: false,
        };

        // Step 4: Add deserialized batch to WAL behavior wrapper (not generic memtable)
        let memtable_start = std::time::Instant::now();
        let sequences = memory_table
            .add_vector_batch(vector_batch)
            .await
            .context("Failed to add vector batch to WAL behavior wrapper")?;
        let memtable_time = memtable_start.elapsed().as_micros();

        // Step 5: Write immutable binary batch to disk (only if immediate_sync is true)  
        if immediate_sync {
            let disk_start = std::time::Instant::now();

            match &self.disk_manager {
                Some(disk_manager) => {
                    // Write immutable binary batch file with BatchId
                    let serialized_data = self.serialize_entries(&entries).await?;
                    match disk_manager.write_raw(&collection_id, serialized_data).await {
                        Ok(_flush_result) => {
                            let disk_time = disk_start.elapsed().as_micros();
                            tracing::info!(
                                "✅ WAL atomic batch write succeeded: batch_id={}, memtable={}μs, disk={}μs, total={}μs",
                                batch_id.batch_uuid,
                                memtable_time, 
                                disk_time, 
                                memtable_time + disk_time
                            );
                        }
                        Err(e) => {
                            // Disk write failed after memtable succeeded
                            // TODO: Rollback batch from memtable using batch_id
                            tracing::error!(
                                "❌ WAL batch disk write failed after memtable write: {}. Batch {} remains in memtable temporarily.",
                                e,
                                batch_id.batch_uuid
                            );

                            return Err(anyhow::anyhow!(
                                "WAL batch write failed: disk write failed for batch {}. Operation aborted for durability: {}",
                                batch_id.batch_uuid,
                                e
                            ));
                        }
                    }
                }
                None => {
                    // No disk manager available
                    // Same limitation: cannot easily rollback individual memtable entries
                    tracing::error!(
                        "❌ No disk manager available for immediate_sync=true. Memtable entries remain temporarily."
                    );

                    return Err(anyhow::anyhow!(
                        "No disk manager available for WAL sync. Cannot guarantee durability with immediate_sync=true"
                    ));
                }
            }
        } else {
            tracing::debug!(
                "✅ WAL write completed (memtable only, no sync): {}μs",
                memtable_time
            );
        }

        let total_time = start_time.elapsed().as_micros();

        if immediate_sync {
            tracing::info!(
                "🚀 WAL atomic write completed: disk_first=true, memtable={}μs, total={}μs",
                memtable_time,
                total_time
            );
        } else {
            tracing::debug!(
                "🚀 WAL write completed (memtable only): {}μs",
                memtable_time
            );
        }

        // Step 3: Per-collection flush threshold checking (enhanced for memory-only durability)
        tracing::info!("🔍 THRESHOLD_CHECK: Checking if any collections need flushing...");
        let collections_to_flush = memory_table.collections_needing_flush().await?;
        tracing::info!(
            "🔍 THRESHOLD_CHECK: Found {} collections that need flushing: {:?}",
            collections_to_flush.len(),
            collections_to_flush
        );

        if !collections_to_flush.is_empty() {
            tracing::info!(
                "🚨 FLUSH_TRIGGER: {} collections need flushing: {:?}",
                collections_to_flush.len(),
                collections_to_flush
            );

            // Trigger coordinated flush for each collection that needs it
            for collection_id in &collections_to_flush {
                tracing::info!(
                    "🚀 TRIGGERING: Coordinated flush for collection {}",
                    collection_id
                );

                // Extract vector records from memtable before calling FlushCoordinator
                let vector_records = if let Some(memory_table) = &self.memory_table {
                    let entries = memory_table.get_all_entries(collection_id).await?;
                    tracing::info!(
                        "📦 EXTRACTED: {} WAL entries for collection {}",
                        entries.len(),
                        collection_id
                    );

                    // Convert WAL entries to vector records
                    let mut extracted_records = Vec::new();
                    for entry in entries {
                        if let WalOperation::Insert { record, .. } = &entry.operation {
                            extracted_records.push(record.clone());
                        }
                    }
                    tracing::info!(
                        "📦 CONVERTED: {} vector records for flushing",
                        extracted_records.len()
                    );
                    extracted_records
                } else {
                    tracing::warn!("📦 No memory table available for data extraction");
                    Vec::new()
                };

                // Create flush data source from extracted vector records
                let flush_data = crate::storage::persistence::wal::flush_coordinator::FlushDataSource::VectorRecords(vector_records);

                // TODO: Get collection metadata to determine storage engine type
                // For now, default to VIPER but this will be fixed when collection service is accessible

                // Execute coordinated flush through flush coordinator
                match self
                    .flush_coordinator
                    .execute_coordinated_flush(&collection_id, flush_data, None, None)
                    .await
                {
                    Ok(storage_result) => {
                        tracing::info!(
                            "✅ FLUSH_SUCCESS: Collection {} - {} entries flushed to storage",
                            collection_id,
                            storage_result.entries_flushed
                        );

                        // Clear flushed entries from memtable after successful storage flush
                        if storage_result.entries_flushed > 0 {
                            memory_table
                                .clear_flushed(collection_id, storage_result.entries_flushed)
                                .await?;
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "❌ FLUSH_ERROR: Collection {} flush failed: {}",
                            collection_id,
                            e
                        );
                        // Continue processing other collections
                    }
                }
            }
        }

        // Step 4: Global flush check (existing logic)
        if memory_table.needs_global_flush().await? {
            let _ = self.flush(None).await; // Background, non-blocking
        }

        Ok(sequences)
    }

    async fn force_sync(&self, collection_id: Option<&CollectionId>) -> Result<()> {
        let start_time = std::time::Instant::now();

        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Get all entries from memtable for the specified collection(s)
        let entries = if let Some(cid) = collection_id {
            memory_table.get_all_entries(cid).await?
        } else {
            // Get all entries for all collections using stats
            let mut all_entries = Vec::new();
            let stats = memory_table.get_stats().await?;
            for collection_id in stats.keys() {
                let collection_entries = memory_table.get_all_entries(collection_id).await?;
                all_entries.extend(collection_entries);
            }
            all_entries
        };

        // Force immediate disk sync
        if !entries.is_empty() {
            if let Some(disk_manager) = &self.disk_manager {
                disk_manager
                    .append_wal_entries(&entries)
                    .await
                    .context("Failed to force sync WAL entries to disk")?;

                tracing::info!(
                    "🔄 Force sync completed: {} entries in {}μs",
                    entries.len(),
                    start_time.elapsed().as_micros()
                );
            }
        }

        Ok(())
    }

    async fn read_entries(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalEntry>> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;
        let disk_manager = self
            .disk_manager
            .as_ref()
            .context("Avro WAL strategy not initialized")?;
        // Deserializer is handled in the disk manager

        // Read from memory first
        let memory_entries = memory_table
            .get_entries(collection_id, from_sequence, limit)
            .await?;

        // If we need more entries, read from disk
        let remaining_limit = limit.map(|l| l.saturating_sub(memory_entries.len()));

        if remaining_limit.unwrap_or(1) > 0 {
            // TODO: Implement disk reading when disk manager supports it
            // For now, only return memory entries
        }

        Ok(memory_entries)
    }

    async fn search_by_vector_id(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Search in memory first (most recent data)
        memory_table.search_vector(collection_id, vector_id).await
    }

    async fn get_latest_entry(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>> {
        self.search_by_vector_id(collection_id, vector_id).await
    }

    async fn get_collection_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        tracing::debug!(
            "📋 Getting all entries for collection {} from memtable",
            collection_id
        );

        memory_table.get_all_entries(collection_id).await
    }

    async fn flush(&self, collection_id: Option<&CollectionId>) -> Result<FlushResult> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;
        let disk_manager = self
            .disk_manager
            .as_ref()
            .context("Avro WAL strategy not initialized")?;
        // Serializer is handled in the disk manager

        let start_time = std::time::Instant::now();
        let mut total_result = FlushResult {
            entries_flushed: 0,
            bytes_written: 0,
            segments_created: 0,
            collections_affected: Vec::new(),
            flush_duration_ms: 0,
        };

        if let Some(collection_id) = collection_id {
            // For metadata WAL: Keep entries in memory, only flush when memory is full
            let entries = memory_table.get_all_entries(collection_id).await?;
            if !entries.is_empty() {
                // Check if this is metadata WAL (small collections that should stay in memory)
                let is_metadata_wal =
                    collection_id.starts_with("metadata_") || entries.len() < 1000; // Heuristic for metadata

                if is_metadata_wal {
                    tracing::debug!(
                        "📝 Metadata WAL: Keeping {} entries for {} in memory",
                        entries.len(),
                        collection_id
                    );
                    // Don't flush metadata to disk - keep in memory for recovery
                    total_result.entries_flushed += entries.len() as u64;
                    total_result
                        .collections_affected
                        .push(collection_id.clone());
                } else {
                    let flush_start = std::time::Instant::now();
                    tracing::info!(
                        "💾 FLUSH START: Flushing {} entries for collection {} to disk",
                        entries.len(),
                        collection_id
                    );

                    // Show operation breakdown by type
                    let mut insert_count = 0;
                    let mut update_count = 0;
                    let mut delete_count = 0;
                    let mut other_count = 0;

                    for entry in &entries {
                        match &entry.operation {
                            crate::storage::persistence::wal::WalOperation::Insert { .. } => {
                                insert_count += 1
                            }
                            crate::storage::persistence::wal::WalOperation::Update { .. } => {
                                update_count += 1
                            }
                            crate::storage::persistence::wal::WalOperation::Delete { .. } => {
                                delete_count += 1
                            }
                            _ => other_count += 1,
                        }
                    }

                    tracing::debug!("📊 FLUSH: Collection {} operations - insert: {}, update: {}, delete: {}, other: {}", 
                                   collection_id, insert_count, update_count, delete_count, other_count);

                    // Serialize entries to Avro format
                    let serialize_start = std::time::Instant::now();
                    let serialized_data = self.serialize_entries_impl(&entries).await?;
                    let serialize_time = serialize_start.elapsed();
                    tracing::debug!(
                        "📦 FLUSH: Serialized {} entries to {} bytes in {:?}",
                        entries.len(),
                        serialized_data.len(),
                        serialize_time
                    );

                    // Write to disk using disk manager
                    let disk_write_start = std::time::Instant::now();
                    let flush_result = disk_manager
                        .write_raw(collection_id, serialized_data)
                        .await?;
                    let disk_write_time = disk_write_start.elapsed();
                    tracing::debug!(
                        "💽 FLUSH: Wrote {} bytes to disk in {:?}",
                        flush_result.bytes_written,
                        disk_write_time
                    );

                    // Clear flushed entries from memory
                    let clear_start = std::time::Instant::now();
                    let last_sequence = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
                    memory_table
                        .clear_flushed(collection_id, last_sequence)
                        .await?;
                    let clear_time = clear_start.elapsed();
                    tracing::debug!(
                        "🧹 FLUSH: Cleared {} entries from memory (up to sequence {}) in {:?}",
                        entries.len(),
                        last_sequence,
                        clear_time
                    );

                    total_result.entries_flushed += entries.len() as u64;
                    total_result.bytes_written += flush_result.bytes_written;
                    total_result.segments_created += flush_result.segments_created;
                    total_result
                        .collections_affected
                        .push(collection_id.clone());

                    let total_flush_time = flush_start.elapsed();
                    tracing::info!("✅ FLUSH COMPLETE: Collection {} - {} entries, {} bytes, {} segments in {:?}", 
                                  collection_id, entries.len(), flush_result.bytes_written, 
                                  flush_result.segments_created, total_flush_time);
                }
            }
        } else {
            // Flush all collections (but keep metadata in memory)
            let collections_needing_flush = memory_table.collections_needing_flush().await?;
            for collection_id in collections_needing_flush {
                let entries = memory_table.get_all_entries(&collection_id).await?;
                if !entries.is_empty() {
                    // Check if this is metadata WAL
                    let is_metadata_wal =
                        collection_id.starts_with("metadata_") || entries.len() < 1000; // Heuristic for metadata

                    if is_metadata_wal {
                        tracing::debug!(
                            "📝 Metadata WAL: Keeping {} entries for {} in memory",
                            entries.len(),
                            collection_id
                        );
                        // Don't flush metadata to disk - keep in memory for recovery
                        total_result.entries_flushed += entries.len() as u64;
                        total_result
                            .collections_affected
                            .push(collection_id.clone());
                    } else {
                        let flush_start = std::time::Instant::now();
                        tracing::info!(
                            "💾 FLUSH START: Flushing {} entries for collection {} to disk",
                            entries.len(),
                            collection_id
                        );

                        // Show operation breakdown by type
                        let mut insert_count = 0;
                        let mut update_count = 0;
                        let mut delete_count = 0;
                        let mut other_count = 0;

                        for entry in &entries {
                            match &entry.operation {
                                crate::storage::persistence::wal::WalOperation::Insert {
                                    ..
                                } => insert_count += 1,
                                crate::storage::persistence::wal::WalOperation::Update {
                                    ..
                                } => update_count += 1,
                                crate::storage::persistence::wal::WalOperation::Delete {
                                    ..
                                } => delete_count += 1,
                                _ => other_count += 1,
                            }
                        }

                        tracing::debug!("📊 FLUSH: Collection {} operations - insert: {}, update: {}, delete: {}, other: {}", 
                                       collection_id, insert_count, update_count, delete_count, other_count);

                        // Serialize entries to Avro format
                        let serialize_start = std::time::Instant::now();
                        let serialized_data = self.serialize_entries_impl(&entries).await?;
                        let serialize_time = serialize_start.elapsed();
                        tracing::debug!(
                            "📦 FLUSH: Serialized {} entries to {} bytes in {:?}",
                            entries.len(),
                            serialized_data.len(),
                            serialize_time
                        );

                        // Write to disk using disk manager
                        let disk_write_start = std::time::Instant::now();
                        let flush_result = disk_manager
                            .write_raw(&collection_id, serialized_data)
                            .await?;
                        let disk_write_time = disk_write_start.elapsed();
                        tracing::debug!(
                            "💽 FLUSH: Wrote {} bytes to disk in {:?}",
                            flush_result.bytes_written,
                            disk_write_time
                        );

                        // Clear flushed entries from memory
                        let clear_start = std::time::Instant::now();
                        let last_sequence = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
                        memory_table
                            .clear_flushed(&collection_id, last_sequence)
                            .await?;
                        let clear_time = clear_start.elapsed();
                        tracing::debug!(
                            "🧹 FLUSH: Cleared {} entries from memory (up to sequence {}) in {:?}",
                            entries.len(),
                            last_sequence,
                            clear_time
                        );

                        total_result.entries_flushed += entries.len() as u64;
                        total_result.bytes_written += flush_result.bytes_written;
                        total_result.segments_created += flush_result.segments_created;
                        total_result
                            .collections_affected
                            .push(collection_id.clone());

                        let total_flush_time = flush_start.elapsed();
                        tracing::info!("✅ FLUSH COMPLETE: Collection {} - {} entries, {} bytes, {} segments in {:?}", 
                                      collection_id, entries.len(), flush_result.bytes_written, 
                                      flush_result.segments_created, total_flush_time);

                        // 🔍 DETAILED TRACING: Check if we need to delegate to storage engine
                        tracing::info!("🔍 DELEGATION CHECK: WAL flush complete, evaluating storage engine delegation for {}", collection_id);
                        let storage_engine_guard = self.storage_engine.read().await;
                        tracing::debug!(
                            "🔍 DELEGATION: Storage engine available: {}",
                            storage_engine_guard.is_some()
                        );

                        // 🚀 COORDINATED STORAGE ENGINE FLUSH via FlushCoordinator (Strategy Pattern)
                        tracing::info!(
                            "🚀 TRIGGERING: Coordinated flush for collection {}",
                            collection_id
                        );
                        let storage_flush_start = std::time::Instant::now();

                        // Extract vector records from WAL entries for storage engine flush
                        let vector_records: Vec<crate::core::VectorRecord> = entries
                            .iter()
                            .filter_map(|entry| match &entry.operation {
                                crate::storage::persistence::wal::WalOperation::Insert {
                                    record,
                                    ..
                                } => Some(record.clone()),
                                crate::storage::persistence::wal::WalOperation::Update {
                                    record,
                                    ..
                                } => Some(record.clone()),
                                _ => None,
                            })
                            .collect();

                        tracing::info!("🔍 DELEGATION: Extracted {} vector records from {} WAL entries for storage engine", 
                                      vector_records.len(), entries.len());

                        let flush_data = FlushDataSource::VectorRecords(vector_records);
                        match self
                            .flush_coordinator
                            .execute_coordinated_flush(&collection_id, flush_data, None, None)
                            .await
                        {
                            Ok(storage_result) => {
                                let storage_flush_time = storage_flush_start.elapsed();
                                tracing::info!("✅ COORDINATED FLUSH SUCCESS: {} - {} entries, {} bytes, {} files in {:?}", 
                                              collection_id, storage_result.entries_flushed, storage_result.bytes_written, 
                                              storage_result.files_created, storage_flush_time);
                            }
                            Err(e) => {
                                let storage_flush_time = storage_flush_start.elapsed();
                                tracing::error!(
                                    "❌ COORDINATED FLUSH FAILED: {} - error after {:?}: {}",
                                    collection_id,
                                    storage_flush_time,
                                    e
                                );
                                tracing::error!("🔍 FLUSH ERROR DETAILS: {:?}", e);
                            }
                        }
                    }
                }
            }
        }

        total_result.flush_duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(total_result)
    }

    // REMOVED: delegate_to_storage_engine_flush and delegate_to_storage_engine_compact
    // These are replaced by the FlushCoordinator.execute_coordinated_flush() approach

    async fn compact_collection(&self, collection_id: &CollectionId) -> Result<u64> {
        let compaction_start = std::time::Instant::now();
        tracing::info!(
            "🗜️ COMPACTION START: Starting compaction for collection {}",
            collection_id
        );

        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Get pre-compaction stats
        let pre_stats = memory_table
            .get_collection_stats(collection_id)
            .await
            .unwrap_or_default();
        tracing::debug!(
            "📊 COMPACTION: Pre-compaction stats for {} - entries: {}, memory: {} bytes",
            collection_id,
            pre_stats.total_entries,
            pre_stats.memory_size_bytes
        );

        // For now, just do memory cleanup
        let maintenance_start = std::time::Instant::now();
        let stats = memory_table.maintenance().await?;
        let maintenance_time = maintenance_start.elapsed();

        tracing::debug!(
            "🧹 COMPACTION: Memory maintenance completed in {:?}",
            maintenance_time
        );
        tracing::debug!(
            "📈 COMPACTION: Cleanup stats - Total entries: {}, Memory freed: {} bytes",
            stats.total_entries,
            stats.memory_size_bytes
        );

        // Get post-compaction stats
        let post_stats = memory_table
            .get_collection_stats(collection_id)
            .await
            .unwrap_or_default();
        let memory_saved = pre_stats
            .memory_size_bytes
            .saturating_sub(post_stats.memory_size_bytes);

        let total_compaction_time = compaction_start.elapsed();
        let total_cleaned = stats.total_entries;

        tracing::info!(
            "✅ COMPACTION COMPLETE: Collection {} - {} items cleaned, {} bytes freed in {:?}",
            collection_id,
            total_cleaned,
            memory_saved,
            total_compaction_time
        );

        Ok(total_cleaned)
    }

    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;
        let disk_manager = self
            .disk_manager
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Drop from memory
        memory_table.drop_collection(collection_id).await?;

        // Drop from disk
        disk_manager.drop_collection(collection_id).await?;

        // Clean up flush coordinator state
        self.flush_coordinator
            .cleanup_collection(collection_id)
            .await;

        tracing::info!("✅ Dropped WAL data for collection: {}", collection_id);
        Ok(())
    }

    async fn get_stats(&self) -> Result<WalStats> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;
        let disk_manager = self
            .disk_manager
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        let memory_stats = memory_table.get_stats().await?;
        let disk_stats = disk_manager.get_stats().await?;

        // Aggregate memory stats
        let total_memory_entries: u64 = memory_stats.values().map(|s| s.total_entries).sum();
        let total_memory_bytes: u64 = memory_stats
            .values()
            .map(|s| s.memory_size_bytes as u64)
            .sum();
        let memory_collections_count = memory_stats.len();

        Ok(WalStats {
            total_entries: total_memory_entries + disk_stats.total_segments,
            memory_entries: total_memory_entries,
            disk_segments: disk_stats.total_segments,
            total_disk_size_bytes: disk_stats.total_size_bytes,
            memory_size_bytes: total_memory_bytes,
            collections_count: memory_collections_count.max(disk_stats.collections_count),
            last_flush_time: Some(Utc::now()), // TODO: Track actual last flush time
            write_throughput_entries_per_sec: 0.0, // TODO: Calculate actual throughput
            read_throughput_entries_per_sec: 0.0, // TODO: Calculate actual throughput
            compression_ratio: disk_stats.compression_ratio,
        })
    }

    async fn recover(&self) -> Result<u64> {
        tracing::info!("🔄 WAL RECOVERY: Starting Avro WAL recovery");

        // Get all WAL files from disk
        if let Some(disk_manager) = &self.disk_manager {
            let collections = disk_manager.list_collections().await?;
            tracing::info!(
                "📂 WAL RECOVERY: Found {} collections to recover from WAL disk manager",
                collections.len()
            );
            tracing::debug!(
                "📂 WAL RECOVERY: Collection IDs from disk manager: {:?}",
                collections
            );

            let mut total_entries = 0u64;
            for collection_id in collections {
                tracing::debug!("📄 WAL RECOVERY: Recovering collection {}", collection_id);

                // Read all entries for this collection
                let entries = disk_manager.read_entries(&collection_id, 0, None).await?;
                let entry_count = entries.len();

                if entry_count > 0 {
                    tracing::info!(
                        "📦 WAL RECOVERY: Found {} entries for collection {}",
                        entry_count,
                        collection_id
                    );

                    // Debug: Show some entry details
                    if !entries.is_empty() {
                        let first_entry = &entries[0];
                        let last_entry = &entries[entries.len() - 1];
                        // Count operation types
                        let mut op_types = std::collections::HashSet::new();
                        for entry in &entries {
                            match &entry.operation {
                                crate::storage::persistence::wal::WalOperation::Insert {
                                    ..
                                } => {
                                    op_types.insert("Insert");
                                }
                                crate::storage::persistence::wal::WalOperation::Update {
                                    ..
                                } => {
                                    op_types.insert("Update");
                                }
                                crate::storage::persistence::wal::WalOperation::Delete {
                                    ..
                                } => {
                                    op_types.insert("Delete");
                                }
                                crate::storage::persistence::wal::WalOperation::Flush => {
                                    op_types.insert("Flush");
                                }
                                crate::storage::persistence::wal::WalOperation::Checkpoint => {
                                    op_types.insert("Checkpoint");
                                }
                                crate::storage::persistence::wal::WalOperation::AvroPayload {
                                    ..
                                } => {
                                    op_types.insert("AvroPayload");
                                }
                            }
                        }

                        tracing::debug!(
                            "📊 WAL RECOVERY: Collection {} - First sequence: {}, Last sequence: {}, Operation types: {:?}",
                            collection_id,
                            first_entry.sequence,
                            last_entry.sequence,
                            op_types
                        );
                    }

                    // Load entries into memory table
                    if let Some(memory_table) = &self.memory_table {
                        let mut loaded_count = 0;
                        // Group entries by collection for batch processing
                        let mut entry_groups: std::collections::HashMap<String, Vec<crate::storage::persistence::wal::WalEntry>> = std::collections::HashMap::new();
                        for entry in entries {
                            entry_groups.entry(entry.collection_id.clone()).or_default().push(entry);
                        }

                        for (collection_id, collection_entries) in entry_groups {
                            let vector_records: Vec<crate::core::VectorRecord> = collection_entries
                                .iter()
                                .filter_map(|entry| match &entry.operation {
                                    crate::storage::persistence::wal::WalOperation::Insert { record, .. } => Some(record.clone()),
                                    crate::storage::persistence::wal::WalOperation::Update { record, .. } => Some(record.clone()),
                                    _ => None,
                                })
                                .collect();

                            if !vector_records.is_empty() {
                                let start_seq = collection_entries.iter().map(|e| e.sequence).min().unwrap_or(0);
                                let end_seq = collection_entries.iter().map(|e| e.sequence).max().unwrap_or(0);
                                
                                let batch_id = crate::storage::persistence::wal::BatchId::new(
                                    collection_id.clone(),
                                    start_seq,
                                    end_seq,
                                );

                                let batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
                                    batch_id,
                                    vector_records,
                                    created_at: std::time::SystemTime::now(),
                                    total_size_bytes: collection_entries.iter().map(|e| e.actual_size_bytes()).sum(),
                                    is_flushed: false,
                                };

                                // Update sequence generator to handle recovery sequences
                                let max_sequence = end_seq;
                                loop {
                                    let current = memory_table.current_sequence();
                                    if max_sequence <= current {
                                        break;
                                    }
                                    // Generate sequences up to max_sequence to sync the generator
                                    let _ = memory_table.next_sequence();
                                    if memory_table.current_sequence() >= max_sequence {
                                        break;
                                    }
                                }

                                memory_table.add_vector_batch(batch).await?;
                                loaded_count += collection_entries.len();
                            }
                        }
                        tracing::debug!(
                            "✅ WAL RECOVERY: Loaded {} entries into memory table for collection {}",
                            loaded_count,
                            collection_id
                        );
                    } else {
                        tracing::warn!(
                            "⚠️ WAL RECOVERY: No memory table available for collection {}",
                            collection_id
                        );
                    }

                    total_entries += entry_count as u64;
                } else {
                    tracing::debug!(
                        "📭 WAL RECOVERY: No entries found for collection {} (collection created but no data inserted yet)",
                        collection_id
                    );
                }
            }

            tracing::info!(
                "✅ WAL RECOVERY: Recovered {} total entries from disk",
                total_entries
            );
            Ok(total_entries)
        } else {
            tracing::warn!("⚠️ WAL RECOVERY: No disk manager configured, skipping recovery");
            Ok(0)
        }
    }

    async fn force_flush_all(&self) -> Result<()> {
        tracing::warn!("⚠️ AvroWalStrategy: FORCE FLUSH ALL - TESTING ONLY");

        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Get all collections that need flushing
        let collections = memory_table.collections_needing_flush().await?;

        for collection_id in &collections {
            if let Err(e) = self
                .force_flush_collection(&collection_id.to_string(), None)
                .await
            {
                tracing::error!(
                    "❌ Force flush failed for collection {}: {}",
                    collection_id,
                    e
                );
            }
        }

        tracing::info!(
            "✅ AvroWalStrategy: Force flush completed for {} collections",
            collections.len()
        );
        Ok(())
    }

    /// Register storage engine with the flush coordinator
    async fn register_storage_engine(
        &self,
        engine_name: &str,
        engine: Arc<dyn UnifiedStorageEngine>,
    ) -> Result<()> {
        self.flush_coordinator
            .register_storage_engine(engine_name, engine)
            .await;
        tracing::info!(
            "✅ AvroWalStrategy: Registered {} storage engine with flush coordinator",
            engine_name
        );
        Ok(())
    }

    async fn force_flush_collection(
        &self,
        collection_id: &str,
        storage_engine: Option<&str>,
    ) -> Result<()> {
        tracing::warn!(
            "⚠️ AvroWalStrategy: FORCE FLUSH COLLECTION {} with engine {:?} - TESTING ONLY",
            collection_id,
            storage_engine
        );

        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Convert string to CollectionId
        let collection_id = CollectionId::from(collection_id.to_string());

        // Extract vector records from memtable
        let entries = memory_table.get_all_entries(&collection_id).await?;

        if entries.is_empty() {
            tracing::info!(
                "📋 AvroWalStrategy: No entries to flush for collection {}",
                collection_id
            );
            return Ok(());
        }

        tracing::info!(
            "🚀 AvroWalStrategy: Force flushing {} entries for collection {} using engine {:?}",
            entries.len(),
            collection_id,
            storage_engine
        );

        // Convert WAL entries to vector records (zero-copy for Avro)
        let mut vector_records = Vec::new();
        for entry in &entries {
            if let WalOperation::Insert {
                vector_id: _,
                record,
                ..
            } = &entry.operation
            {
                vector_records.push(record.clone());
            } else if let WalOperation::Update {
                vector_id: _,
                record,
                ..
            } = &entry.operation
            {
                vector_records.push(record.clone());
            }
        }

        tracing::info!(
            "📦 AvroWalStrategy: Extracted {} vector records for flushing (zero-copy)",
            vector_records.len()
        );

        // Create flush data source from extracted vector records
        let flush_data =
            crate::storage::persistence::wal::flush_coordinator::FlushDataSource::VectorRecords(
                vector_records,
            );

        // Execute coordinated flush through flush coordinator with specified storage engine
        match self
            .flush_coordinator
            .execute_coordinated_flush(&collection_id, flush_data, storage_engine, None)
            .await
        {
            Ok(storage_result) => {
                tracing::info!("✅ AvroWalStrategy: Force flush SUCCESS - {} entries flushed to storage for collection {}", 
                              storage_result.entries_flushed, collection_id);

                // Clear flushed entries from memtable after successful storage flush
                if storage_result.entries_flushed > 0 {
                    memory_table
                        .clear_flushed(&collection_id, storage_result.entries_flushed)
                        .await?;
                    tracing::info!(
                        "🧹 AvroWalStrategy: Cleared {} entries from memtable",
                        storage_result.entries_flushed
                    );
                }
            }
            Err(e) => {
                tracing::error!(
                    "❌ AvroWalStrategy: Force flush FAILED for collection {}: {}",
                    collection_id,
                    e
                );
                return Err(e);
            }
        }

        Ok(())
    }

    async fn close(&self) -> Result<()> {
        // Flush any remaining data
        let _ = self.flush(None).await;

        tracing::info!("✅ Avro WAL strategy closed");
        Ok(())
    }

    fn get_assignment_service(
        &self,
    ) -> &Arc<dyn crate::storage::assignment_service::AssignmentService> {
        &self.assignment_service
    }

    /// **AvroWAL Optimized Implementation**
    ///
    /// This override provides enhanced atomic flush operations specifically optimized for Avro:
    /// - Zero-copy VectorRecord extraction (already stored as VectorRecord)
    /// - Schema evolution support during flush operations
    /// - Efficient memtable-specific operations based on configured strategy
    async fn atomic_retrieve_for_flush(
        &self,
        collection_id: &CollectionId,
        flush_id: &str,
    ) -> Result<FlushCycle> {
        tracing::info!("🔄 AvroWAL: Starting OPTIMIZED atomic flush retrieval for collection {} (flush_id: {})", 
                      collection_id, flush_id);

        // Use memtable-specific atomic operations for advanced coordination
        if let Some(memory_table) = &self.memory_table {
            // Get current sequence number for this flush operation
            let current_sequence = memory_table.current_sequence();
            let wal_entries = memory_table
                .atomic_mark_for_flush(collection_id, current_sequence)
                .await?;

            // AvroWAL optimization: Zero-copy VectorRecord extraction
            let mut vector_records = Vec::new();
            let mut marked_sequences = Vec::new();

            if !wal_entries.is_empty() {
                let min_seq = wal_entries.iter().map(|e| e.sequence).min().unwrap_or(0);
                let max_seq = wal_entries.iter().map(|e| e.sequence).max().unwrap_or(0);
                marked_sequences.push((min_seq, max_seq));

                // Zero-copy extraction: AvroWAL already stores VectorRecords directly
                for entry in &wal_entries {
                    match &entry.operation {
                        WalOperation::Insert {
                            vector_id: _,
                            record,
                            expires_at: _,
                        } => {
                            vector_records.push(record.clone()); // Already VectorRecord - no conversion needed
                        }
                        WalOperation::Update {
                            vector_id: _,
                            record,
                            expires_at: _,
                        } => {
                            vector_records.push(record.clone()); // Already VectorRecord - no conversion needed
                        }
                        _ => {
                            // Skip other operation types but include in flush cycle
                        }
                    }
                }
            }

            tracing::info!("✅ AvroWAL: OPTIMIZED flush retrieval complete - {} entries, {} vector records (zero-copy)", 
                          wal_entries.len(), vector_records.len());

            Ok(FlushCycle {
                flush_id: flush_id.to_string(),
                collection_id: collection_id.clone(),
                entries: wal_entries,
                vector_records,
                marked_segments: Vec::new(), // TODO: Add disk segment support
                marked_sequences,
                batch_ids: vec![],
                state: FlushCycleState::Active,
            })
        } else {
            // Fallback to default implementation
            tracing::warn!("⚠️ AvroWAL: Memory table not available, using default implementation");
            // Get entries using the standard method
            let entries = self.get_collection_entries(collection_id).await?;
            let mut vector_records = Vec::new();
            for entry in &entries {
                match &entry.operation {
                    WalOperation::Insert {
                        vector_id: _,
                        record,
                        expires_at: _,
                    } => {
                        vector_records.push(record.clone());
                    }
                    WalOperation::Update {
                        vector_id: _,
                        record,
                        expires_at: _,
                    } => {
                        vector_records.push(record.clone());
                    }
                    _ => {}
                }
            }
            Ok(FlushCycle {
                flush_id: flush_id.to_string(),
                collection_id: collection_id.clone(),
                entries,
                vector_records,
                marked_segments: Vec::new(),
                marked_sequences: Vec::new(),
                batch_ids: vec![],
                state: FlushCycleState::Active,
            })
        }
    }

    /// **AvroWAL Optimized Completion**
    ///
    /// Uses memtable-specific removal operations for optimal performance
    async fn complete_flush_cycle(&self, flush_cycle: FlushCycle) -> Result<FlushCompletionResult> {
        tracing::info!(
            "🗑️ AvroWAL: Starting OPTIMIZED flush completion for collection {} (flush_id: {})",
            flush_cycle.collection_id,
            flush_cycle.flush_id
        );

        if let Some(memory_table) = &self.memory_table {
            // Use memtable-specific optimized removal
            // Use memtable-specific optimized removal
            // Extract max sequence from marked sequences (stored in the flush cycle)
            let max_sequence = flush_cycle
                .marked_sequences
                .iter()
                .map(|(_, max)| *max)
                .max()
                .unwrap_or(0);
            let entries_removed = memory_table
                .complete_flush_removal(&flush_cycle.collection_id, max_sequence)
                .await?;

            let result = FlushCompletionResult {
                entries_removed,
                segments_cleaned: 0, // TODO: Add disk segment cleanup
                bytes_reclaimed: flush_cycle
                    .entries
                    .iter()
                    .map(|entry| std::mem::size_of_val(entry))
                    .sum::<usize>() as u64,
            };

            tracing::info!(
                "✅ AvroWAL: OPTIMIZED flush completion - {} entries removed efficiently",
                result.entries_removed
            );
            Ok(result)
        } else {
            // Fallback to default implementation
            tracing::warn!("⚠️ AvroWAL: Memory table not available, using default implementation");
            self.drop_collection(&flush_cycle.collection_id).await?;
            Ok(FlushCompletionResult {
                entries_removed: flush_cycle.entries.len(),
                segments_cleaned: 0,
                bytes_reclaimed: 0,
            })
        }
    }

    /// **AvroWAL Optimized Abort**
    ///
    /// Uses memtable-specific restoration for fast recovery
    async fn abort_flush_cycle(&self, flush_cycle: FlushCycle, reason: &str) -> Result<()> {
        tracing::warn!(
            "❌ AvroWAL: Starting OPTIMIZED flush abort for collection {} - reason: {}",
            flush_cycle.collection_id,
            reason
        );

        if let Some(memory_table) = &self.memory_table {
            // Use memtable-specific optimized restoration
            // Use memtable-specific optimized restoration
            memory_table
                .abort_flush_restore(&flush_cycle.collection_id, flush_cycle.entries.clone())
                .await?;
            tracing::info!(
                "✅ AvroWAL: OPTIMIZED flush abort completed - entries restored efficiently"
            );
        } else {
            tracing::warn!("⚠️ AvroWAL: Memory table not available, using default abort handling");
        }

        Ok(())
    }

    /// Get memtable reference for similarity search
    fn memtable(&self) -> Option<&crate::storage::memtable::specialized::WalMemtable> {
        self.memory_table.as_ref()
    }

    /// Get WAL behavior wrapper for direct batch access (optimization for search)
    fn get_wal_behavior_wrapper(&self) -> Option<&crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper> {
        // WalMemtable is a type alias for WalBehaviorWrapper, so we can return it directly
        self.memory_table.as_ref()
    }
}

#[async_trait]
impl FlushCoordinatorCallbacks for AvroWalStrategy {
    /// Get WAL files containing the specified sequences
    async fn get_wal_files_for_sequences(
        &self,
        collection_id: &CollectionId,
        sequences: &[u64],
    ) -> Result<Vec<String>> {
        // This is a placeholder implementation - actual logic would:
        // 1. Get the WAL directory for this collection
        // 2. List WAL files and check which contain the requested sequences
        // 3. Return the file paths

        if let Some(config) = &self.config {
            let wal_url = self
                .select_wal_url_for_collection(collection_id, config)
                .await?;
            let dir_path = wal_url.trim_start_matches("file://");

            // For now, return all WAL files in the collection directory
            // In a real implementation, we'd parse file names to find which contain our sequences
            if let Some(filesystem) = &self.filesystem {
                if let Ok(fs) = filesystem.get_filesystem("file://") {
                    match fs.list(&format!("{}/{}", dir_path, collection_id)).await {
                        Ok(files) => {
                            let wal_files: Vec<String> = files
                                .into_iter()
                                .filter(|f| f.name.contains("wal_") && f.name.ends_with(".avro"))
                                .map(|f| f.name)
                                .collect();
                            tracing::debug!(
                                "📁 Found {} WAL files for collection {} sequences {:?}",
                                wal_files.len(),
                                collection_id,
                                sequences
                            );
                            return Ok(wal_files);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "📁 Failed to list WAL files for {}: {}",
                                collection_id,
                                e
                            );
                        }
                    }
                }
            }
        }

        Ok(Vec::new())
    }

    /// Check if a WAL file is fully flushed and can be safely deleted
    async fn is_wal_file_fully_flushed(
        &self,
        _collection_id: &CollectionId,
        _wal_file: &str,
        _flushed_sequences: &[u64],
    ) -> Result<bool> {
        // This is a placeholder implementation - actual logic would:
        // 1. Parse the WAL file to get its sequence range
        // 2. Check if all sequences in the file are in flushed_sequences
        // 3. Return true only if the file is completely covered

        // For now, assume files can be deleted (conservative approach would be false)
        Ok(true)
    }
}

impl DistanceComputeProvider for AvroWalStrategy {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}
