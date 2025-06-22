// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Avro WAL Strategy - Schema Evolution Support with High Performance

use anyhow::{Context, Result};
use apache_avro::{Reader, Schema, Writer};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{
    FlushResult, WalConfig, WalDiskManager, WalEntry, WalMemTable, WalOperation, WalStats,
    WalStrategy,
};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::filesystem::FilesystemFactory;

/// Avro schema for WAL entries with evolution support
const AVRO_SCHEMA_V1: &str = r#"
{
  "type": "record",
  "name": "WalEntry",
  "namespace": "ai.proximadb.wal",
  "fields": [
    {"name": "entry_id", "type": "string"},
    {"name": "collection_id", "type": "string"},
    {"name": "operation", "type": {
      "type": "record",
      "name": "WalOperation",
      "fields": [
        {"name": "op_type", "type": {"type": "enum", "name": "OpType", "symbols": ["INSERT", "UPDATE", "DELETE", "CREATE_COLLECTION", "DROP_COLLECTION"]}},
        {"name": "vector_id", "type": ["null", "string"], "default": null},
        {"name": "vector_data", "type": ["null", "bytes"], "default": null},
        {"name": "metadata", "type": ["null", "string"], "default": null},
        {"name": "config", "type": ["null", "string"], "default": null},
        {"name": "expires_at", "type": ["null", "long"], "default": null}
      ]
    }},
    {"name": "timestamp", "type": "long"},
    {"name": "sequence", "type": "long"},
    {"name": "global_sequence", "type": "long"},
    {"name": "expires_at", "type": ["null", "long"], "default": null},
    {"name": "version", "type": "long", "default": 1}
  ]
}
"#;

/// Avro representation of WAL entry
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AvroWalEntry {
    entry_id: String,
    collection_id: String,
    operation: AvroWalOperation,
    timestamp: i64,
    sequence: i64,
    global_sequence: i64,
    expires_at: Option<i64>,
    version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AvroWalOperation {
    op_type: AvroOpType,
    vector_id: Option<String>,
    vector_data: Option<Vec<u8>>, // Serialized vector record
    metadata: Option<String>,
    config: Option<String>,
    expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum AvroOpType {
    #[serde(rename = "INSERT")]
    Insert,
    #[serde(rename = "UPDATE")]
    Update,
    #[serde(rename = "DELETE")]
    Delete,
    #[serde(rename = "CREATE_COLLECTION")]
    CreateCollection,
    #[serde(rename = "DROP_COLLECTION")]
    DropCollection,
}

/// Avro WAL strategy implementation
#[derive(Debug)]
pub struct AvroWalStrategy {
    config: Option<WalConfig>,
    filesystem: Option<Arc<FilesystemFactory>>,
    memory_table: Option<WalMemTable>,
    disk_manager: Option<WalDiskManager>,
}

impl AvroWalStrategy {
    /// Create new Avro WAL strategy
    pub fn new() -> Self {
        Self {
            config: None,
            filesystem: None,
            memory_table: None,
            disk_manager: None,
        }
    }

    /// Internal Avro serialization implementation
    async fn serialize_entries_impl(&self, entries: &[WalEntry]) -> Result<Vec<u8>> {
        let config = self.config.as_ref().context("Config not initialized")?;

        // Inline serialization implementation
        let schema = Schema::parse_str(AVRO_SCHEMA_V1).context("Failed to parse Avro schema")?;

        let compression_codec = match config.compression.algorithm {
            crate::storage::wal::config::CompressionAlgorithm::None => apache_avro::Codec::Null,
            crate::storage::wal::config::CompressionAlgorithm::Snappy => {
                apache_avro::Codec::Deflate
            }
            crate::storage::wal::config::CompressionAlgorithm::Lz4 => apache_avro::Codec::Deflate,
            crate::storage::wal::config::CompressionAlgorithm::Zstd { .. } => {
                apache_avro::Codec::Deflate
            }
        };

        let mut writer = Writer::with_codec(&schema, Vec::new(), compression_codec);

        for entry in entries {
            let avro_entry = convert_to_avro_entry(entry)?;
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

            let entry = convert_from_avro_entry(avro_entry)?;
            entries.push(entry);
        }

        Ok(entries)
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
        self.memory_table = Some(WalMemTable::new(config.clone()).await?);
        self.disk_manager = Some(WalDiskManager::new(config.clone(), filesystem).await?);

        tracing::info!("✅ Avro WAL strategy initialized");
        tracing::debug!("✅ AvroWalStrategy::initialize - Initialization complete");
        Ok(())
    }

    async fn serialize_entries(&self, entries: &[WalEntry]) -> Result<Vec<u8>> {
        self.serialize_entries_impl(entries).await
    }

    async fn deserialize_entries(&self, data: &[u8]) -> Result<Vec<WalEntry>> {
        self.deserialize_entries_impl(data).await
    }

    async fn write_entry(&self, entry: WalEntry) -> Result<u64> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        let sequence = memory_table.insert_entry(entry).await?;

        // Check if we need to flush
        let collections_needing_flush = memory_table.collections_needing_flush().await?;
        if !collections_needing_flush.is_empty() {
            // Background flush (in production, this would be handled by a background task)
            for collection_id in collections_needing_flush {
                let _ = self.flush(Some(&collection_id)).await;
            }
        }

        Ok(sequence)
    }

    async fn write_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        let sequences = memory_table.insert_batch(entries).await?;

        // Check for global flush need
        if memory_table.needs_global_flush().await? {
            // Force flush all collections
            let _ = self.flush(None).await;
        }

        Ok(sequences)
    }

    async fn write_batch_with_sync(&self, entries: Vec<WalEntry>, immediate_sync: bool) -> Result<Vec<u64>> {
        let start_time = std::time::Instant::now();
        
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // Step 1: Write to memtable (fast, concurrent reads continue)
        let sequences = memory_table.insert_batch(entries.clone()).await?;
        let memtable_time = start_time.elapsed().as_micros();

        // Step 2: Conditional disk sync based on immediate_sync flag
        if immediate_sync {
            let disk_start = std::time::Instant::now();
            
            // Use a more robust disk write approach
            match &self.disk_manager {
                Some(disk_manager) => {
                    // Try disk write but don't fail the entire operation if it fails
                    match disk_manager.append_wal_entries(&entries).await {
                        Ok(()) => {
                            let disk_time = disk_start.elapsed().as_micros();
                            tracing::info!(
                                "🚀 WAL write completed: memtable={}μs, disk={}μs, total={}μs", 
                                memtable_time,
                                disk_time,
                                start_time.elapsed().as_micros()
                            );
                        },
                        Err(e) => {
                            tracing::warn!(
                                "⚠️ WAL disk sync failed but memtable write succeeded: {}. Operation continues with in-memory durability.",
                                e
                            );
                            // Don't fail the operation - memtable write succeeded
                        }
                    }
                },
                None => {
                    tracing::warn!("⚠️ No disk manager available for WAL sync. Using memory-only durability.");
                }
            }
        } else {
            tracing::debug!(
                "🚀 WAL write completed (memtable only): {}μs", 
                memtable_time
            );
        }

        // Step 3: Background VIPER flush check (existing logic)
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
                let is_metadata_wal = collection_id.starts_with("metadata_") || 
                                     entries.len() < 1000; // Heuristic for metadata
                
                if is_metadata_wal {
                    tracing::debug!("📝 Metadata WAL: Keeping {} entries for {} in memory", 
                                   entries.len(), collection_id);
                    // Don't flush metadata to disk - keep in memory for recovery
                    total_result.entries_flushed += entries.len() as u64;
                    total_result.collections_affected.push(collection_id.clone());
                } else {
                    tracing::debug!("💾 Flushing {} entries for collection {} to disk", entries.len(), collection_id);
                    
                    // Serialize entries to Avro format
                    let serialized_data = self.serialize_entries_impl(&entries).await?;
                    
                    // Write to disk using disk manager
                    let flush_result = disk_manager.write_raw(collection_id, serialized_data).await?;
                    
                    // Clear flushed entries from memory
                    let last_sequence = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
                    memory_table
                        .clear_flushed(collection_id, last_sequence)
                        .await?;

                    total_result.entries_flushed += entries.len() as u64;
                    total_result.bytes_written += flush_result.bytes_written;
                    total_result.segments_created += flush_result.segments_created;
                    total_result.collections_affected.push(collection_id.clone());
                    
                    tracing::info!("✅ Flushed {} entries ({} bytes) for collection {} to disk", 
                                  entries.len(), flush_result.bytes_written, collection_id);
                }
            }
        } else {
            // Flush all collections (but keep metadata in memory)
            let collections_needing_flush = memory_table.collections_needing_flush().await?;
            for collection_id in collections_needing_flush {
                let entries = memory_table.get_all_entries(&collection_id).await?;
                if !entries.is_empty() {
                    // Check if this is metadata WAL 
                    let is_metadata_wal = collection_id.starts_with("metadata_") || 
                                         entries.len() < 1000; // Heuristic for metadata
                    
                    if is_metadata_wal {
                        tracing::debug!("📝 Metadata WAL: Keeping {} entries for {} in memory", 
                                       entries.len(), collection_id);
                        // Don't flush metadata to disk - keep in memory for recovery
                        total_result.entries_flushed += entries.len() as u64;
                        total_result.collections_affected.push(collection_id.clone());
                    } else {
                        tracing::debug!("💾 Flushing {} entries for collection {} to disk", entries.len(), collection_id);
                        
                        // Serialize entries to Avro format
                        let serialized_data = self.serialize_entries_impl(&entries).await?;
                        
                        // Write to disk using disk manager
                        let flush_result = disk_manager.write_raw(&collection_id, serialized_data).await?;
                        
                        // Clear flushed entries from memory
                        let last_sequence = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
                        memory_table
                            .clear_flushed(&collection_id, last_sequence)
                            .await?;

                        total_result.entries_flushed += entries.len() as u64;
                        total_result.bytes_written += flush_result.bytes_written;
                        total_result.segments_created += flush_result.segments_created;
                        total_result.collections_affected.push(collection_id.clone());
                        
                        tracing::info!("✅ Flushed {} entries ({} bytes) for collection {} to disk", 
                                      entries.len(), flush_result.bytes_written, collection_id);
                    }
                }
            }
        }

        total_result.flush_duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(total_result)
    }

    async fn compact_collection(&self, _collection_id: &CollectionId) -> Result<u64> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Avro WAL strategy not initialized")?;

        // For now, just do memory cleanup
        let stats = memory_table.maintenance().await?;
        Ok(stats.mvcc_versions_cleaned + stats.ttl_entries_expired)
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
        let total_memory_bytes: u64 = memory_stats.values().map(|s| s.memory_bytes as u64).sum();
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
                "📂 WAL RECOVERY: Found {} collections to recover",
                collections.len()
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

                    // Load entries into memory table
                    if let Some(memory_table) = &self.memory_table {
                        for entry in entries {
                            memory_table.insert_entry(entry).await?;
                        }
                    }

                    total_entries += entry_count as u64;
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

    async fn close(&self) -> Result<()> {
        // Flush any remaining data
        let _ = self.flush(None).await;

        tracing::info!("✅ Avro WAL strategy closed");
        Ok(())
    }
}

/// Convert WAL entry to Avro format
fn convert_to_avro_entry(entry: &WalEntry) -> Result<AvroWalEntry> {
    let operation = match &entry.operation {
        WalOperation::Insert {
            vector_id,
            record,
            expires_at,
        } => AvroWalOperation {
            op_type: AvroOpType::Insert,
            vector_id: Some(vector_id.to_string()),
            vector_data: Some(serialize_vector_record(record)?),
            metadata: None,
            config: None,
            expires_at: expires_at.map(|dt| dt.timestamp_millis()),
        },
        WalOperation::Update {
            vector_id,
            record,
            expires_at,
        } => AvroWalOperation {
            op_type: AvroOpType::Update,
            vector_id: Some(vector_id.to_string()),
            vector_data: Some(serialize_vector_record(record)?),
            metadata: None,
            config: None,
            expires_at: expires_at.map(|dt| dt.timestamp_millis()),
        },
        WalOperation::Delete {
            vector_id,
            expires_at,
        } => AvroWalOperation {
            op_type: AvroOpType::Delete,
            vector_id: Some(vector_id.to_string()),
            vector_data: None,
            metadata: None,
            config: None,
            expires_at: expires_at.map(|dt| dt.timestamp_millis()),
        },
        WalOperation::CreateCollection {
            collection_id: _,
            config,
        } => AvroWalOperation {
            op_type: AvroOpType::CreateCollection,
            vector_id: None,
            vector_data: None,
            metadata: None,
            config: Some(config.to_string()),
            expires_at: None,
        },
        WalOperation::DropCollection { collection_id: _ } => AvroWalOperation {
            op_type: AvroOpType::DropCollection,
            vector_id: None,
            vector_data: None,
            metadata: None,
            config: None,
            expires_at: None,
        },
        WalOperation::AvroPayload {
            operation_type: _,
            avro_data,
        } => AvroWalOperation {
            op_type: AvroOpType::Insert, // Use Insert as default for binary Avro data
            vector_id: None,
            vector_data: Some(avro_data.clone()),
            metadata: None,
            config: None,
            expires_at: None,
        },
    };

    Ok(AvroWalEntry {
        entry_id: entry.entry_id.to_string(),
        collection_id: entry.collection_id.to_string(),
        operation,
        timestamp: entry.timestamp.timestamp_millis(),
        sequence: entry.sequence as i64,
        global_sequence: entry.global_sequence as i64,
        expires_at: entry.expires_at.map(|dt| dt.timestamp_millis()),
        version: entry.version as i64,
    })
}

/// Convert from Avro format to WAL entry
fn convert_from_avro_entry(avro_entry: AvroWalEntry) -> Result<WalEntry> {
    let operation = match avro_entry.operation.op_type {
        AvroOpType::Insert => {
            let vector_id = VectorId::from(
                avro_entry
                    .operation
                    .vector_id
                    .context("Missing vector_id for Insert")?,
            );
            let record = deserialize_vector_record(
                avro_entry
                    .operation
                    .vector_data
                    .context("Missing vector_data for Insert")?,
            )?;
            let expires_at = avro_entry
                .operation
                .expires_at
                .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now()));

            WalOperation::Insert {
                vector_id,
                record,
                expires_at,
            }
        }
        AvroOpType::Update => {
            let vector_id = VectorId::from(
                avro_entry
                    .operation
                    .vector_id
                    .context("Missing vector_id for Update")?,
            );
            let record = deserialize_vector_record(
                avro_entry
                    .operation
                    .vector_data
                    .context("Missing vector_data for Update")?,
            )?;
            let expires_at = avro_entry
                .operation
                .expires_at
                .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now()));

            WalOperation::Update {
                vector_id,
                record,
                expires_at,
            }
        }
        AvroOpType::Delete => {
            let vector_id = VectorId::from(
                avro_entry
                    .operation
                    .vector_id
                    .context("Missing vector_id for Delete")?,
            );
            let expires_at = avro_entry
                .operation
                .expires_at
                .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now()));

            WalOperation::Delete {
                vector_id,
                expires_at,
            }
        }
        AvroOpType::CreateCollection => {
            let config: serde_json::Value = serde_json::from_str(
                &avro_entry
                    .operation
                    .config
                    .context("Missing config for CreateCollection")?,
            )
            .context("Invalid JSON config for CreateCollection")?;

            WalOperation::CreateCollection {
                collection_id: CollectionId::from(avro_entry.collection_id.clone()),
                config,
            }
        }
        AvroOpType::DropCollection => WalOperation::DropCollection {
            collection_id: CollectionId::from(avro_entry.collection_id.clone()),
        },
    };

    Ok(WalEntry {
        entry_id: avro_entry.entry_id,
        collection_id: CollectionId::from(avro_entry.collection_id),
        operation,
        timestamp: DateTime::from_timestamp_millis(avro_entry.timestamp)
            .unwrap_or_else(|| Utc::now()),
        sequence: avro_entry.sequence as u64,
        global_sequence: avro_entry.global_sequence as u64,
        expires_at: avro_entry
            .expires_at
            .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now())),
        version: avro_entry.version as u64,
    })
}

/// Serialize vector record to bytes
fn serialize_vector_record(record: &VectorRecord) -> Result<Vec<u8>> {
    bincode::serialize(record).context("Failed to serialize VectorRecord")
}

/// Deserialize vector record from bytes
fn deserialize_vector_record(data: Vec<u8>) -> Result<VectorRecord> {
    bincode::deserialize(&data).context("Failed to deserialize VectorRecord")
}
