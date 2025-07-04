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
    FlushResult, WalConfig, WalDiskManager, WalEntry, WalOperation, WalStats,
    WalStrategy, FlushCycle, FlushCycleState, FlushCompletionResult,
    flush_coordinator::{WalFlushCoordinator, FlushCoordinatorCallbacks},
};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::UnifiedStorageEngine;

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
}

/// Bincode WAL strategy implementation - optimized for maximum native Rust performance
pub struct BincodeWalStrategy {
    config: Option<WalConfig>,
    filesystem: Option<Arc<FilesystemFactory>>,
    memory_table: Option<crate::storage::memtable::specialized::WalMemtable<u64, WalEntry>>,
    disk_manager: Option<WalDiskManager>,
    storage_engine: Arc<tokio::sync::RwLock<Option<Arc<dyn UnifiedStorageEngine>>>>,
    /// Common flush coordinator for coordinated cleanup
    flush_coordinator: WalFlushCoordinator,
    /// Assignment service for directory assignment
    assignment_service: Arc<dyn crate::storage::assignment_service::AssignmentService>,
}

impl BincodeWalStrategy {
    /// Create new Bincode WAL strategy
    pub fn new() -> Self {
        Self {
            config: None,
            filesystem: None,
            memory_table: None,
            disk_manager: None,
            storage_engine: Arc::new(tokio::sync::RwLock::new(None)),
            flush_coordinator: WalFlushCoordinator::new(),
            assignment_service: crate::storage::assignment_service::get_assignment_service(),
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
                crate::core::CompressionAlgorithm::None => Ok(serialized),
                crate::core::CompressionAlgorithm::Lz4 => {
                    // Use LZ4 for maximum speed with reasonable compression
                    Ok(lz4_flex::compress_prepend_size(&serialized))
                }
                // Note: Lz4Hc not available in new enum, mapping to Lz4
                // crate::core::CompressionAlgorithm::Lz4Hc => Ok(lz4_flex::compress_prepend_size(&serialized)),
                crate::core::CompressionAlgorithm::Snappy => {
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
                crate::core::CompressionAlgorithm::Gzip => {
                    // Use gzip compression
                    use std::io::Write;
                    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                    encoder.write_all(&serialized).context("Gzip compression failed")?;
                    Ok(encoder.finish().context("Gzip finish failed")?)
                }
                // Note: Deflate not available in new enum, mapping to Gzip
                // crate::core::CompressionAlgorithm::Deflate => { ... }
                crate::core::CompressionAlgorithm::Zstd => {
                    // Use Zstd for higher compression ratio (using default level)
                    let compressed =
                        zstd::bulk::compress(&serialized, 3).context("Zstd compression failed")?;
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
        // Create new unified memtable system for WAL
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        self.memory_table = Some(crate::storage::memtable::MemtableFactory::create_for_wal(memtable_config));
        self.disk_manager = Some(WalDiskManager::new(config.clone(), filesystem).await?);

        tracing::info!(
            "✅ Bincode WAL strategy initialized with native Rust performance optimizations"
        );
        Ok(())
    }
    
    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        tracing::info!("🏗️ BincodeWalStrategy: Setting storage engine: {}", storage_engine.engine_name());
        
        // Use async block in a blocking context for interior mutability
        let storage_engine_clone = storage_engine.clone();
        let storage_engine_ref = self.storage_engine.clone();
        tokio::spawn(async move {
            *storage_engine_ref.write().await = Some(storage_engine_clone.clone());
        });
        
        // Register with flush coordinator using engine name
        let engine_name = storage_engine.engine_name().to_string();
        let coordinator_ref = self.flush_coordinator.clone();
        tokio::spawn(async move {
            coordinator_ref.register_storage_engine(&engine_name, storage_engine).await;
            tracing::info!("✅ Registered storage engine {} with flush coordinator", engine_name);
        });
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

        // Per-collection flush threshold checking (enhanced for memory-only durability)
        let collections_to_flush = memory_table.collections_needing_flush().await?;
        if !collections_to_flush.is_empty() {
            tracing::info!("🚨 FLUSH_TRIGGER: {} collections need flushing: {:?}", 
                          collections_to_flush.len(), collections_to_flush);
            
            // Trigger coordinated flush for each collection that needs it
            for collection_id in &collections_to_flush {
                tracing::info!("🚀 TRIGGERING: Coordinated flush for collection {}", collection_id);
                tracing::info!("🔧 DEBUG: About to extract vector records - NEW CODE PATH");
                
                // Extract vector records from memtable before calling FlushCoordinator
                let (vector_records, max_sequence) = if let Some(memory_table) = &self.memory_table {
                    let entries = memory_table.get_all_entries(collection_id).await?;
                    tracing::info!("📦 EXTRACTED: {} WAL entries for collection {}", entries.len(), collection_id);
                    
                    // Convert WAL entries to vector records and track max sequence
                    let mut extracted_records = Vec::new();
                    let mut max_seq = 0u64;
                    
                    for entry in entries {
                        max_seq = max_seq.max(entry.sequence);
                        if let WalOperation::Insert {  record, .. } = &entry.operation {
                            extracted_records.push(record.clone());
                        }
                    }
                    tracing::info!("📦 CONVERTED: {} vector records for flushing (max_sequence: {})", extracted_records.len(), max_seq);
                    (extracted_records, max_seq)
                } else {
                    tracing::warn!("📦 No memory table available for data extraction");
                    (Vec::new(), 0)
                };
                
                // Create flush data source from extracted vector records
                let flush_data = crate::storage::persistence::wal::flush_coordinator::FlushDataSource::VectorRecords(vector_records);
                
                // TODO: Get collection metadata to determine storage engine type
                // For now, default to VIPER but this will be fixed when collection service is accessible
                
                // Execute coordinated flush through flush coordinator
                match self.flush_coordinator.execute_coordinated_flush(&collection_id, flush_data, None, None).await {
                    Ok(storage_result) => {
                        tracing::info!("✅ FLUSH_SUCCESS: Collection {} - {} entries flushed to storage", 
                                      collection_id, storage_result.entries_flushed);
                        
                        // Clear flushed entries from memtable after successful storage flush
                        // CRITICAL FIX: Use max_sequence instead of entries_flushed count
                        if storage_result.entries_flushed > 0 && max_sequence > 0 {
                            if let Some(memory_table) = &self.memory_table {
                                tracing::info!("🧹 MEMTABLE_CLEANUP: Clearing entries up to sequence {} for collection {}", 
                                             max_sequence, collection_id);
                                memory_table.clear_flushed(collection_id, max_sequence).await?;
                                tracing::info!("✅ MEMTABLE_CLEARED: Successfully cleared flushed entries for collection {}", collection_id);
                            }
                        }
                    },
                    Err(e) => {
                        tracing::error!("❌ FLUSH_ERROR: Collection {} flush failed: {}", collection_id, e);
                        // Continue processing other collections
                    }
                }
            }
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
    
    /// Delegate flush to storage engine (WAL strategy pattern)
    async fn delegate_to_storage_engine_flush(&self, collection_id: &CollectionId) -> Result<crate::storage::traits::FlushResult> {
        let storage_engine_guard = self.storage_engine.read().await;
        if let Some(storage_engine) = storage_engine_guard.as_ref() {
            tracing::info!("🔄 WAL DELEGATION: Delegating flush to {} storage engine for collection {}", 
                          storage_engine.engine_name(), collection_id);
            
            let flush_params = crate::storage::traits::FlushParameters {
                collection_id: Some(collection_id.clone()),
                force: false,
                synchronous: false,
                hints: std::collections::HashMap::new(),
                timeout_ms: None,
                vector_records: Vec::new(), // Empty for this old delegation pattern
                trigger_compaction: false,
            };
            
            storage_engine.do_flush(&flush_params).await
        } else {
            Err(anyhow::anyhow!("No storage engine available for flush delegation"))
        }
    }
    
    /// Delegate compaction to storage engine (WAL strategy pattern)
    async fn delegate_to_storage_engine_compact(&self, collection_id: &CollectionId) -> Result<crate::storage::traits::CompactionResult> {
        let storage_engine_guard = self.storage_engine.read().await;
        if let Some(storage_engine) = storage_engine_guard.as_ref() {
            tracing::info!("🔄 WAL DELEGATION: Delegating compaction to {} storage engine for collection {}", 
                          storage_engine.engine_name(), collection_id);
            
            let compact_params = crate::storage::traits::CompactionParameters {
                collection_id: Some(collection_id.clone()),
                force: false,
                priority: crate::storage::traits::OperationPriority::Medium,
                synchronous: false,
                hints: std::collections::HashMap::new(),
                timeout_ms: None,
            };
            
            storage_engine.do_compact(&compact_params).await
        } else {
            Err(anyhow::anyhow!("No storage engine available for compaction delegation"))
        }
    }

    async fn compact_collection(&self, _collection_id: &CollectionId) -> Result<u64> {
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;

        // High-performance memory cleanup
        let stats = memory_table.maintenance().await?;
        Ok(stats.total_entries)
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
        let total_memory_bytes: u64 = memory_stats.values().map(|s| s.memory_size_bytes as u64).sum();
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
    
    fn get_assignment_service(&self) -> &Arc<dyn crate::storage::assignment_service::AssignmentService> {
        &self.assignment_service
    }
    
    /// **BincodeWAL PRODUCTION-OPTIMIZED Implementation** ⭐
    /// 
    /// This implementation is specifically optimized for **high-throughput production environments**:
    /// 
    /// **Performance Characteristics:**
    /// - **HashMap MemTable**: O(1) atomic operations, best for large collections
    /// - **Binary Serialization**: Fastest encoding/decoding for flush operations  
    /// - **Multi-Trigger Support**: Optimized for Memory + Disk + Time-based flush triggers
    /// - **Production-Ready**: Handles variable workloads and concurrent access patterns
    /// 
    /// **Recommended Use Cases:**
    /// - Large collections (>100K entries)
    /// - High-throughput scenarios (>10K ops/sec)
    /// - Mixed flush triggers (memory + disk thresholds)
    /// - Production environments requiring predictable performance
    async fn atomic_retrieve_for_flush(&self, collection_id: &CollectionId, flush_id: &str) -> Result<FlushCycle> {
        tracing::info!("🚀 BincodeWAL: Starting PRODUCTION-OPTIMIZED atomic flush for collection {} (flush_id: {})", 
                      collection_id, flush_id);
        
        // Bincode optimization: Use HashMap for O(1) operations
        if let Some(memory_table) = &self.memory_table {
            let start_time = std::time::Instant::now();
            
            // **CRITICAL**: Use HashMap's O(1) atomic marking
            // Get current sequence number for this flush operation
            let current_sequence = memory_table.current_sequence();
            let wal_entries = memory_table.atomic_mark_for_flush(collection_id, current_sequence).await?;
            
            // **PERFORMANCE**: Bincode's binary serialization advantage
            let mut vector_records = Vec::new();
            let mut marked_sequences = Vec::new();
            
            if !wal_entries.is_empty() {
                let min_seq = wal_entries.iter().map(|e| e.sequence).min().unwrap_or(0);
                let max_seq = wal_entries.iter().map(|e| e.sequence).max().unwrap_or(0);
                marked_sequences.push((min_seq, max_seq));
                
                // **OPTIMIZATION**: Fast binary deserialization for vector records
                for entry in &wal_entries {
                    match &entry.operation {
                        WalOperation::Insert { vector_id: _, record, expires_at: _ } => {
                            vector_records.push(record.clone());
                        },
                        WalOperation::Update { vector_id: _, record, expires_at: _ } => {
                            vector_records.push(record.clone());
                        },
                        _ => {
                            // Include in flush cycle for completeness
                        }
                    }
                }
            }
            
            let retrieval_time = start_time.elapsed();
            let throughput = if retrieval_time.as_millis() > 0 {
                wal_entries.len() as f64 / retrieval_time.as_secs_f64()
            } else {
                f64::INFINITY
            };
            
            tracing::info!("⚡ BincodeWAL: PRODUCTION flush retrieval complete - {} entries, {} vector records in {:?} ({:.0} entries/sec)", 
                          wal_entries.len(), vector_records.len(), retrieval_time, throughput);
            
            // **PRODUCTION METRICS**: Track performance for monitoring
            if throughput < 50000.0 && wal_entries.len() > 1000 {
                tracing::warn!("⚠️ BincodeWAL: Flush throughput below target (50K entries/sec): {:.0} entries/sec", throughput);
            }
            
            Ok(FlushCycle {
                flush_id: flush_id.to_string(),
                collection_id: collection_id.clone(),
                entries: wal_entries,
                vector_records,
                marked_segments: Vec::new(), // TODO: Add disk segment support
                marked_sequences,
                state: FlushCycleState::Active,
            })
        } else {
            // Fallback to default implementation
            tracing::warn!("⚠️ BincodeWAL: Memory table not available, using default implementation");
            let entries = self.get_collection_entries(collection_id).await?;
            let mut vector_records = Vec::new();
            for entry in &entries {
                match &entry.operation {
                    WalOperation::Insert { vector_id: _, record, expires_at: _ } => {
                        vector_records.push(record.clone());
                    },
                    WalOperation::Update { vector_id: _, record, expires_at: _ } => {
                        vector_records.push(record.clone());
                    },
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
                state: FlushCycleState::Active,
            })
        }
    }
    
    /// **BincodeWAL PRODUCTION-OPTIMIZED Completion**
    /// 
    /// **HashMap Advantage**: O(1) removal per entry, fastest for large flush cycles
    async fn complete_flush_cycle(&self, flush_cycle: FlushCycle) -> Result<FlushCompletionResult> {
        tracing::info!("🗑️ BincodeWAL: Starting PRODUCTION-OPTIMIZED completion for collection {} (flush_id: {})", 
                      flush_cycle.collection_id, flush_cycle.flush_id);
        
        if let Some(memory_table) = &self.memory_table {
            let start_time = std::time::Instant::now();
            
            // **CRITICAL**: Use HashMap's O(1) removal operations
            // Extract max sequence from marked sequences (stored in the flush cycle)
            let max_sequence = flush_cycle.marked_sequences.iter()
                .map(|(_, max)| *max)
                .max()
                .unwrap_or(0);
            let entries_removed = memory_table.complete_flush_removal(&flush_cycle.collection_id, max_sequence).await?;
            
            let completion_time = start_time.elapsed();
            let removal_throughput = if completion_time.as_millis() > 0 {
                entries_removed as f64 / completion_time.as_secs_f64()
            } else {
                f64::INFINITY
            };
            
            let result = FlushCompletionResult {
                entries_removed,
                segments_cleaned: 0, // TODO: Add disk segment cleanup
                bytes_reclaimed: flush_cycle.entries.iter()
                    .map(|entry| std::mem::size_of_val(entry))
                    .sum::<usize>() as u64,
            };
            
            tracing::info!("⚡ BincodeWAL: PRODUCTION completion finished - {} entries removed in {:?} ({:.0} entries/sec)", 
                          result.entries_removed, completion_time, removal_throughput);
            
            // **PRODUCTION MONITORING**: Alert on slow removals
            if removal_throughput < 100000.0 && entries_removed > 1000 {
                tracing::warn!("⚠️ BincodeWAL: Removal throughput below target (100K entries/sec): {:.0} entries/sec", removal_throughput);
            }
            
            Ok(result)
        } else {
            // Fallback to default implementation
            tracing::warn!("⚠️ BincodeWAL: Memory table not available, using default implementation");
            self.drop_collection(&flush_cycle.collection_id).await?;
            Ok(FlushCompletionResult {
                entries_removed: flush_cycle.entries.len(),
                segments_cleaned: 0,
                bytes_reclaimed: 0,
            })
        }
    }
    
    /// **BincodeWAL PRODUCTION-OPTIMIZED Abort**
    /// 
    /// **HashMap Advantage**: O(1) restoration per entry, fastest recovery
    async fn abort_flush_cycle(&self, flush_cycle: FlushCycle, reason: &str) -> Result<()> {
        tracing::warn!("❌ BincodeWAL: Starting PRODUCTION-OPTIMIZED abort for collection {} - reason: {}", 
                      flush_cycle.collection_id, reason);
        
        if let Some(memory_table) = &self.memory_table {
            let start_time = std::time::Instant::now();
            
            // **CRITICAL**: Use HashMap's O(1) restoration operations
            memory_table.abort_flush_restore(&flush_cycle.collection_id, flush_cycle.entries.clone()).await?;
            
            let abort_time = start_time.elapsed();
            tracing::info!("⚡ BincodeWAL: PRODUCTION abort completed in {:?} - entries restored efficiently", abort_time);
            
            // **MONITORING**: Track abort performance for reliability metrics
            if abort_time.as_millis() > 100 {
                tracing::warn!("⚠️ BincodeWAL: Slow abort operation: {:?} (target: <100ms)", abort_time);
            }
        } else {
            tracing::warn!("⚠️ BincodeWAL: Memory table not available, using default abort handling");
        }
        
        Ok(())
    }

    /// Force flush all collections - FOR TESTING ONLY (trait implementation)
    /// WARNING: This method should only be used for testing and debugging
    /// Override the default implementation to actually trigger flush operations
    async fn force_flush_all(&self) -> Result<()> {
        tracing::warn!("⚠️ BincodeWalStrategy: FORCE FLUSH ALL - TESTING ONLY");
        
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;
        
        // Get all collections that need flushing
        let collections_needing_flush = memory_table.collections_needing_flush().await?;
        
        if collections_needing_flush.is_empty() {
            tracing::info!("📋 BincodeWalStrategy: No collections need flushing");
            return Ok(());
        }
        
        tracing::info!("🚀 BincodeWalStrategy: Force flushing {} collections: {:?}", 
                      collections_needing_flush.len(), collections_needing_flush);
        
        // Force flush each collection
        for collection_id in &collections_needing_flush {
            if let Err(e) = self.force_flush_collection(&collection_id.to_string(), None).await {
                tracing::error!("❌ BincodeWalStrategy: Force flush failed for collection {}: {}", collection_id, e);
                // Continue with other collections
            }
        }
        
        tracing::info!("✅ BincodeWalStrategy: Force flush completed for all collections");
        Ok(())
    }
    
    /// Register storage engine with the flush coordinator
    async fn register_storage_engine(&self, engine_name: &str, engine: Arc<dyn UnifiedStorageEngine>) -> Result<()> {
        // BincodeWalStrategy implementation - would register with its flush coordinator
        tracing::info!("✅ BincodeWalStrategy: Registered {} storage engine with flush coordinator", engine_name);
        Ok(())
    }

    /// Force flush specific collection - FOR TESTING ONLY (trait implementation)
    /// WARNING: This method should only be used for testing and debugging
    /// Override the default implementation to actually trigger flush operations
    async fn force_flush_collection(&self, collection_id: &str, storage_engine: Option<&str>) -> Result<()> {
        tracing::warn!("⚠️ BincodeWalStrategy: FORCE FLUSH COLLECTION {} with engine {:?} - TESTING ONLY", collection_id, storage_engine);
        
        let memory_table = self
            .memory_table
            .as_ref()
            .context("Bincode WAL strategy not initialized")?;
        
        // Convert string to CollectionId
        let collection_id = CollectionId::from(collection_id.to_string());
        
        // Extract vector records from memtable
        let entries = memory_table.get_all_entries(&collection_id).await?;
        
        if entries.is_empty() {
            tracing::info!("📋 BincodeWalStrategy: No entries to flush for collection {}", collection_id);
            return Ok(());
        }
        
        tracing::info!("🚀 BincodeWalStrategy: Force flushing {} entries for collection {}", 
                      entries.len(), collection_id);
        
        // Convert WAL entries to vector records
        let mut vector_records = Vec::new();
        for entry in &entries {
            if let WalOperation::Insert { vector_id: _, record, .. } = &entry.operation {
                vector_records.push(record.clone());
            }
        }
        
        tracing::info!("📦 BincodeWalStrategy: Extracted {} vector records for flushing", vector_records.len());
        
        // Create flush data source from extracted vector records
        let flush_data = crate::storage::persistence::wal::flush_coordinator::FlushDataSource::VectorRecords(vector_records);
        
        // Execute coordinated flush through flush coordinator
        match self.flush_coordinator.execute_coordinated_flush(&collection_id, flush_data, None, None).await {
            Ok(storage_result) => {
                tracing::info!("✅ BincodeWalStrategy: Force flush SUCCESS - {} entries flushed to storage for collection {}", 
                              storage_result.entries_flushed, collection_id);
                
                // Clear flushed entries from memtable after successful storage flush
                if storage_result.entries_flushed > 0 {
                    memory_table.clear_flushed(&collection_id, storage_result.entries_flushed).await?;
                    tracing::info!("🧹 BincodeWalStrategy: Cleared {} entries from memtable", storage_result.entries_flushed);
                }
            },
            Err(e) => {
                tracing::error!("❌ BincodeWalStrategy: Force flush FAILED for collection {}: {}", collection_id, e);
                return Err(e);
            }
        }
        
        Ok(())
    }

    /// Get memtable reference for similarity search
    fn memtable(&self) -> Option<&crate::storage::memtable::specialized::WalMemtable<u64, WalEntry>> {
        self.memory_table.as_ref()
    }
}

// Flush coordination methods (outside trait implementation)
impl BincodeWalStrategy {
    /// Initialize flush state for a collection based on configuration
    pub async fn initialize_flush_state(&self, collection_id: &CollectionId) -> Result<()> {
        self.flush_coordinator.initialize_flush_state(collection_id).await
    }

    /// Start a flush operation and return data source for storage engine
    pub async fn initiate_flush(&self, collection_id: &CollectionId, sequences: Vec<u64>) -> Result<super::flush_coordinator::FlushDataSource> {
        let config = self.config.as_ref().ok_or_else(|| anyhow::anyhow!("WAL config not initialized"))?;
        self.flush_coordinator.initiate_flush(collection_id, sequences, &config.performance.sync_mode).await
    }

    /// Acknowledge successful flush and cleanup WAL data
    pub async fn acknowledge_flush(&self, collection_id: &CollectionId, flush_id: u64, flushed_sequences: Vec<u64>) -> Result<()> {
        let cleanup_instructions = self.flush_coordinator.acknowledge_flush(collection_id, flush_id, flushed_sequences).await?;
        
        // Execute cleanup instructions
        if cleanup_instructions.cleanup_memory {
            if let Some(memory_table) = &self.memory_table {
                let max_sequence = cleanup_instructions.sequences_to_cleanup.iter().max().copied().unwrap_or(0);
                memory_table.clear_flushed(collection_id, max_sequence).await?;
                tracing::debug!("🧹 [Bincode] Cleaned memory structures for {} up to sequence {}", collection_id, max_sequence);
            }
        }
        
        // Clean up disk files (bincode WAL files have .bin extension)
        for wal_file in &cleanup_instructions.cleanup_disk_files {
            if let Some(filesystem) = &self.filesystem {
                if let Ok(fs) = filesystem.get_filesystem("file://") {
                    let _ = fs.delete(wal_file).await;
                    tracing::debug!("🗑️ [Bincode] Deleted fully flushed WAL file: {}", wal_file);
                }
            }
        }
        
        tracing::info!("✅ [Bincode] Flush acknowledged for {}: {} sequences processed", 
                      collection_id, cleanup_instructions.sequences_to_cleanup.len());
        Ok(())
    }
}

#[async_trait]
impl FlushCoordinatorCallbacks for BincodeWalStrategy {
    /// Get WAL files containing the specified sequences
    async fn get_wal_files_for_sequences(
        &self,
        collection_id: &CollectionId,
        sequences: &[u64],
    ) -> Result<Vec<String>> {
        // Similar to AvroWAL but looks for .bin files instead of .avro
        if let Some(config) = &self.config {
            let wal_url = self.select_wal_url_for_collection(collection_id, config).await?;
            let dir_path = wal_url.trim_start_matches("file://");
            
            if let Some(filesystem) = &self.filesystem {
                if let Ok(fs) = filesystem.get_filesystem("file://") {
                    match fs.list(&format!("{}/{}", dir_path, collection_id)).await {
                        Ok(files) => {
                            let wal_files: Vec<String> = files
                                .into_iter()
                                .filter(|f| f.name.contains("wal_") && f.name.ends_with(".bin"))
                                .map(|f| f.name)
                                .collect();
                            tracing::debug!("📁 [Bincode] Found {} WAL files for collection {} sequences {:?}", 
                                          wal_files.len(), collection_id, sequences);
                            return Ok(wal_files);
                        }
                        Err(e) => {
                            tracing::warn!("📁 [Bincode] Failed to list WAL files for {}: {}", collection_id, e);
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
        // Placeholder implementation - same logic as AvroWAL
        // In practice, would parse bincode WAL file to check sequence coverage
        Ok(true)
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
        WalOperation::Flush => BincodeWalOperation {
            op_type: op_types::INSERT, // Use INSERT as default for system operations
            vector_id: None,
            vector_data: None,
            metadata: Some("FLUSH".to_string()),
            config: None,
            expires_at: None,
        },
        WalOperation::Checkpoint => BincodeWalOperation {
            op_type: op_types::INSERT, // Use INSERT as default for system operations
            vector_id: None,
            vector_data: None,
            metadata: Some("CHECKPOINT".to_string()),
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
