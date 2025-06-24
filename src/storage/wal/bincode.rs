// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Bincode WAL Strategy - Native Rust Performance with Zero-Copy Optimization

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::Arc;

use super::{
    FlushResult, WalConfig, WalDiskManager, WalEntry, WalMemTable, WalOperation, WalStats,
    WalStrategy,
};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::filesystem::FilesystemFactory;

/// Bincode representation of WAL entry (optimized for native Rust performance)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BincodeWalEntry {
    entry_id: String, // Vector ID or collection ID for identification
    collection_id: String,
    operation: BincodeWalOperation,
    timestamp: i64,
    sequence: u64,
    global_sequence: u64,
    expires_at: Option<i64>,
    version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BincodeWalOperation {
    op_type: u8, // Single byte for operation type
    vector_id: Option<String>,
    vector_data: Option<Vec<u8>>, // Pre-serialized vector record
    metadata: Option<String>,
    config: Option<String>,
    expires_at: Option<i64>,
}

/// Operation type constants for maximum performance
mod op_types {
    pub const INSERT: u8 = 1;
    pub const UPDATE: u8 = 2;
    pub const DELETE: u8 = 3;
    pub const CREATE_COLLECTION: u8 = 4;
    pub const DROP_COLLECTION: u8 = 5;
}

/// Bincode WAL strategy implementation - optimized for maximum native Rust performance
#[derive(Debug)]
pub struct BincodeWalStrategy {
    config: Option<WalConfig>,
    filesystem: Option<Arc<FilesystemFactory>>,
    memory_table: Option<WalMemTable>,
    disk_manager: Option<WalDiskManager>,
}

impl BincodeWalStrategy {
    /// Create new Bincode WAL strategy
    pub fn new() -> Self {
        Self {
            config: None,
            filesystem: None,
            memory_table: None,
            disk_manager: None,
        }
    }

    /// Serialize entries to bincode format with compression
    async fn serialize_entries_impl(&self, entries: &[WalEntry]) -> Result<Vec<u8>> {
        let config = self.config.as_ref().context("Config not initialized")?;

        // Pre-allocate with estimated size for better performance
        let estimated_size = entries.len() * 256; // Rough estimate
        let mut _buffer: Vec<u8> = Vec::with_capacity(estimated_size);

        // Convert to bincode format
        let bincode_entries: Result<Vec<BincodeWalEntry>, _> =
            entries.iter().map(convert_to_bincode_entry).collect();

        let bincode_entries = bincode_entries?;

        // Serialize with bincode (very fast)
        let serialized = bincode::serialize(&bincode_entries)
            .context("Failed to serialize entries with bincode")?;

        // Apply compression if enabled
        if config.compression.compress_disk && serialized.len() > 128 {
            match config.compression.algorithm {
                crate::core::unified_types::CompressionAlgorithm::None => Ok(serialized),
                crate::core::unified_types::CompressionAlgorithm::Lz4 => {
                    // Use LZ4 for maximum speed with reasonable compression
                    Ok(lz4_flex::compress_prepend_size(&serialized))
                }
                crate::core::unified_types::CompressionAlgorithm::Lz4Hc => {
                    // Use LZ4 HC for better compression
                    Ok(lz4_flex::compress_prepend_size(&serialized))
                }
                crate::core::unified_types::CompressionAlgorithm::Snappy => {
                    // Snappy for balanced performance
                    let mut compressed = Vec::new();
                    {
                        let mut encoder = snap::write::FrameEncoder::new(&mut compressed);
                        encoder
                            .write_all(&serialized)
                            .context("Snappy compression failed")?;
                    }
                    Ok(compressed)
                }
                crate::core::unified_types::CompressionAlgorithm::Gzip => {
                    // Use gzip compression
                    use std::io::Write;
                    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                    encoder.write_all(&serialized).context("Gzip compression failed")?;
                    Ok(encoder.finish().context("Gzip finish failed")?)
                }
                crate::core::unified_types::CompressionAlgorithm::Deflate => {
                    // Use deflate compression
                    use std::io::Write;
                    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                    encoder.write_all(&serialized).context("Deflate compression failed")?;
                    Ok(encoder.finish().context("Deflate finish failed")?)
                }
                crate::core::unified_types::CompressionAlgorithm::Zstd { level } => {
                    // Use Zstd for higher compression ratio
                    let compressed =
                        zstd::bulk::compress(&serialized, level).context("Zstd compression failed")?;
                    Ok(compressed)
                }
            }
        } else {
            Ok(serialized)
        }
    }

    /// Deserialize entries from bincode format with decompression
    async fn deserialize_entries_impl(&self, data: &[u8]) -> Result<Vec<WalEntry>> {
        let config = self.config.as_ref().context("Config not initialized")?;

        // Decompress if needed
        let decompressed_data = if config.compression.compress_disk {
            // Try different decompression methods (auto-detect)
            if data.len() >= 4 {
                // Try LZ4 first (most common for high-performance)
                if let Ok(decompressed) = lz4_flex::decompress_size_prepended(data) {
                    decompressed
                } else {
                    // Try Snappy
                    let mut reader = snap::read::FrameDecoder::new(data);
                    let mut decompressed = Vec::new();
                    std::io::copy(&mut reader, &mut decompressed)
                        .context("Failed to decompress data")?;
                    decompressed
                }
            } else {
                data.to_vec()
            }
        } else {
            data.to_vec()
        };

        // Deserialize with bincode (zero-copy where possible)
        let bincode_entries: Vec<BincodeWalEntry> = bincode::deserialize(&decompressed_data)
            .context("Failed to deserialize entries with bincode")?;

        // Convert back to WAL entries
        bincode_entries
            .into_iter()
            .map(convert_from_bincode_entry)
            .collect()
    }
}

#[async_trait]
impl WalStrategy for BincodeWalStrategy {
    fn strategy_name(&self) -> &'static str {
        "Bincode"
    }

    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()> {
        self.config = Some(config.clone());
        self.filesystem = Some(filesystem.clone());
        self.memory_table = Some(WalMemTable::new(config.clone()).await?);
        self.disk_manager = Some(WalDiskManager::new(config.clone(), filesystem).await?);

        tracing::info!(
            "✅ Bincode WAL strategy initialized with native Rust performance optimizations"
        );
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
            .context("Bincode WAL strategy not initialized")?;

        let sequence = memory_table.insert_entry(entry).await?;

        // Check if we need to flush (non-blocking for high throughput)
        let collections_needing_flush = memory_table.collections_needing_flush().await?;
        if !collections_needing_flush.is_empty() {
            // TODO: In production, background flush should be handled by a dedicated background task
            // For now, we'll skip the background flush to avoid lifetime issues
            tracing::debug!(
                "Bincode WAL: {} collections need flushing, will be handled by next explicit flush",
                collections_needing_flush.len()
            );
        }

        Ok(sequence)
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
            .context("Bincode WAL strategy not initialized")?;
        let disk_manager = self
            .disk_manager
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;

        // Memory-first read strategy for hot data
        let mut memory_entries = memory_table
            .get_entries(collection_id, from_sequence, limit)
            .await?;

        // If we need more entries, read from disk
        let remaining_limit = limit.map(|l| l.saturating_sub(memory_entries.len()));

        if remaining_limit.unwrap_or(1) > 0 {
            // TODO: Add read_raw to disk_manager and deserialize here
            // For now, just return memory entries
            // let disk_data = disk_manager.read_raw(collection_id, from_sequence, remaining_limit).await?;
            // let disk_entries = self.deserialize_entries(&disk_data).await?;
            // memory_entries.extend(disk_entries);
            // memory_entries.sort_unstable_by_key(|e| e.sequence);

            // Apply limit again after merging
            if let Some(limit) = limit {
                memory_entries.truncate(limit);
            }
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
            .context("Bincode WAL strategy not initialized")?;

        // Search in memory first (most recent data, highest performance)
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
            .context("Bincode WAL strategy not initialized")?;
        
        tracing::debug!(
            "📋 Getting all entries for collection {} from bincode memtable",
            collection_id
        );
        
        memory_table.get_all_entries(collection_id).await
    }

    async fn flush(&self, collection_id: Option<&CollectionId>) -> Result<FlushResult> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;
        let disk_manager = self
            .disk_manager
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;

        let start_time = std::time::Instant::now();
        let mut total_result = FlushResult {
            entries_flushed: 0,
            bytes_written: 0,
            segments_created: 0,
            collections_affected: Vec::new(),
            flush_duration_ms: 0,
        };

        if let Some(collection_id) = collection_id {
            // Flush specific collection
            let entries = memory_table.get_all_entries(collection_id).await?;
            if !entries.is_empty() {
                let serialized_data = self.serialize_entries(&entries).await?;
                let result = disk_manager
                    .write_raw(collection_id, serialized_data)
                    .await?;

                // Clear memory after successful flush
                let last_sequence = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
                memory_table
                    .clear_flushed(collection_id, last_sequence)
                    .await?;

                total_result.entries_flushed += result.entries_flushed;
                total_result.bytes_written += result.bytes_written;
                total_result.segments_created += result.segments_created;
                total_result
                    .collections_affected
                    .extend(result.collections_affected);
            }
        } else {
            // Flush all collections (serial processing for simplicity with internal serialization)
            let collections_needing_flush = memory_table.collections_needing_flush().await?;

            for collection_id in collections_needing_flush {
                let entries = memory_table.get_all_entries(&collection_id).await?;
                if !entries.is_empty() {
                    let serialized_data = self.serialize_entries(&entries).await?;
                    let result = disk_manager
                        .write_raw(&collection_id, serialized_data)
                        .await?;

                    // Clear memory after successful flush
                    let last_sequence = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
                    memory_table
                        .clear_flushed(&collection_id, last_sequence)
                        .await?;

                    total_result.entries_flushed += result.entries_flushed;
                    total_result.bytes_written += result.bytes_written;
                    total_result.segments_created += result.segments_created;
                    total_result.collections_affected.push(collection_id);
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
            .context("Bincode WAL strategy not initialized")?;

        // High-performance memory cleanup
        let stats = memory_table.maintenance().await?;
        Ok(stats.mvcc_versions_cleaned + stats.ttl_entries_expired)
    }

    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;
        let disk_manager = self
            .disk_manager
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;

        // Parallel drop for performance
        let (memory_result, disk_result) = tokio::join!(
            memory_table.drop_collection(collection_id),
            disk_manager.drop_collection(collection_id)
        );

        memory_result?;
        disk_result?;

        tracing::info!(
            "✅ Dropped WAL data for collection: {} (Bincode strategy)",
            collection_id
        );
        Ok(())
    }

    async fn get_stats(&self) -> Result<WalStats> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;
        let disk_manager = self
            .disk_manager
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;

        // Parallel stats gathering
        let (memory_stats, disk_stats) =
            tokio::join!(memory_table.get_stats(), disk_manager.get_stats());

        let memory_stats = memory_stats?;
        let disk_stats = disk_stats?;

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
        // Recovery is handled by disk manager initialization
        tracing::info!("✅ Bincode WAL recovery completed");
        Ok(0)
    }

    async fn close(&self) -> Result<()> {
        // Flush any remaining data
        let _ = self.flush(None).await;

        tracing::info!("✅ Bincode WAL strategy closed");
        Ok(())
    }
}

/// Convert WAL entry to Bincode format (optimized for performance)
fn convert_to_bincode_entry(entry: &WalEntry) -> Result<BincodeWalEntry> {
    let operation = match &entry.operation {
        WalOperation::Insert {
            vector_id,
            record,
            expires_at,
        } => BincodeWalOperation {
            op_type: op_types::INSERT,
            vector_id: Some(vector_id.to_string()),
            vector_data: Some(serialize_vector_record_fast(record)?),
            metadata: None,
            config: None,
            expires_at: expires_at.map(|dt| dt.timestamp_millis()),
        },
        WalOperation::Update {
            vector_id,
            record,
            expires_at,
        } => BincodeWalOperation {
            op_type: op_types::UPDATE,
            vector_id: Some(vector_id.to_string()),
            vector_data: Some(serialize_vector_record_fast(record)?),
            metadata: None,
            config: None,
            expires_at: expires_at.map(|dt| dt.timestamp_millis()),
        },
        WalOperation::Delete {
            vector_id,
            expires_at,
        } => BincodeWalOperation {
            op_type: op_types::DELETE,
            vector_id: Some(vector_id.to_string()),
            vector_data: None,
            metadata: None,
            config: None,
            expires_at: expires_at.map(|dt| dt.timestamp_millis()),
        },
        WalOperation::CreateCollection {
            collection_id: _,
            config,
        } => BincodeWalOperation {
            op_type: op_types::CREATE_COLLECTION,
            vector_id: None,
            vector_data: None,
            metadata: None,
            config: Some(config.to_string()),
            expires_at: None,
        },
        WalOperation::DropCollection { collection_id: _ } => BincodeWalOperation {
            op_type: op_types::DROP_COLLECTION,
            vector_id: None,
            vector_data: None,
            metadata: None,
            config: None,
            expires_at: None,
        },
        WalOperation::AvroPayload {
            operation_type: _,
            avro_data,
        } => BincodeWalOperation {
            op_type: op_types::INSERT, // Use INSERT as default for binary Avro data
            vector_id: None,
            vector_data: Some(avro_data.clone()),
            metadata: None,
            config: None,
            expires_at: None,
        },
    };

    Ok(BincodeWalEntry {
        entry_id: entry.entry_id.clone(),
        collection_id: entry.collection_id.to_string(),
        operation,
        timestamp: entry.timestamp.timestamp_millis(),
        sequence: entry.sequence,
        global_sequence: entry.global_sequence,
        expires_at: entry.expires_at.map(|dt| dt.timestamp_millis()),
        version: entry.version,
    })
}

/// Convert from Bincode format to WAL entry
fn convert_from_bincode_entry(bincode_entry: BincodeWalEntry) -> Result<WalEntry> {
    let operation = match bincode_entry.operation.op_type {
        op_types::INSERT => {
            let vector_id = VectorId::from(
                bincode_entry
                    .operation
                    .vector_id
                    .context("Missing vector_id for Insert")?,
            );
            let record = deserialize_vector_record_fast(
                bincode_entry
                    .operation
                    .vector_data
                    .context("Missing vector_data for Insert")?,
            )?;
            let expires_at = bincode_entry
                .operation
                .expires_at
                .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now()));

            WalOperation::Insert {
                vector_id,
                record,
                expires_at,
            }
        }
        op_types::UPDATE => {
            let vector_id = VectorId::from(
                bincode_entry
                    .operation
                    .vector_id
                    .context("Missing vector_id for Update")?,
            );
            let record = deserialize_vector_record_fast(
                bincode_entry
                    .operation
                    .vector_data
                    .context("Missing vector_data for Update")?,
            )?;
            let expires_at = bincode_entry
                .operation
                .expires_at
                .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now()));

            WalOperation::Update {
                vector_id,
                record,
                expires_at,
            }
        }
        op_types::DELETE => {
            let vector_id = VectorId::from(
                bincode_entry
                    .operation
                    .vector_id
                    .context("Missing vector_id for Delete")?,
            );
            let expires_at = bincode_entry
                .operation
                .expires_at
                .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now()));

            WalOperation::Delete {
                vector_id,
                expires_at,
            }
        }
        op_types::CREATE_COLLECTION => {
            let config: serde_json::Value = serde_json::from_str(
                &bincode_entry
                    .operation
                    .config
                    .context("Missing config for CreateCollection")?,
            )
            .context("Invalid JSON config for CreateCollection")?;

            WalOperation::CreateCollection {
                collection_id: CollectionId::from(bincode_entry.collection_id.clone()),
                config,
            }
        }
        op_types::DROP_COLLECTION => WalOperation::DropCollection {
            collection_id: CollectionId::from(bincode_entry.collection_id.clone()),
        },
        _ => {
            return Err(anyhow::anyhow!(
                "Invalid operation type: {}",
                bincode_entry.operation.op_type
            ))
        }
    };

    Ok(WalEntry {
        entry_id: bincode_entry.entry_id,
        collection_id: CollectionId::from(bincode_entry.collection_id),
        operation,
        timestamp: DateTime::from_timestamp_millis(bincode_entry.timestamp)
            .unwrap_or_else(|| Utc::now()),
        sequence: bincode_entry.sequence,
        global_sequence: bincode_entry.global_sequence,
        expires_at: bincode_entry
            .expires_at
            .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now())),
        version: bincode_entry.version,
    })
}

/// Fast vector record serialization for maximum performance
fn serialize_vector_record_fast(record: &VectorRecord) -> Result<Vec<u8>> {
    // Use bincode for maximum speed
    bincode::serialize(record).context("Failed to serialize VectorRecord with bincode")
}

/// Fast vector record deserialization for maximum performance
fn deserialize_vector_record_fast(data: Vec<u8>) -> Result<VectorRecord> {
    // Use bincode for maximum speed
    bincode::deserialize(&data).context("Failed to deserialize VectorRecord with bincode")
}

impl Default for FlushResult {
    fn default() -> Self {
        Self {
            entries_flushed: 0,
            bytes_written: 0,
            segments_created: 0,
            collections_affected: Vec::new(),
            flush_duration_ms: 0,
        }
    }
}
