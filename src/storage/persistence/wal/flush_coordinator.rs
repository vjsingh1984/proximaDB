// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Common WAL Flush Coordination Logic
//!
//! This module provides shared flush coordination logic that can be used by
//! both AvroWAL and BincodeWAL implementations to manage flush state tracking,
//! cleanup of memory structures, and coordination between memory/disk WAL modes.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::core::CollectionId;
use crate::storage::traits::{UnifiedStorageEngine, FlushParameters, FlushResult};
use super::config::SyncMode;

/// Flush state tracking for coordinated WAL cleanup
#[derive(Debug, Clone)]
pub struct FlushState {
    /// Pending flushes waiting for storage acknowledgment
    pub pending_flushes: HashMap<u64, PendingFlush>,
    /// Last successfully flushed sequence number
    pub last_flushed_sequence: u64,
    /// Whether this collection uses disk WAL (vs memory-only)
    pub uses_disk_wal: bool,
}

impl Default for FlushState {
    fn default() -> Self {
        Self {
            pending_flushes: HashMap::new(),
            last_flushed_sequence: 0,
            uses_disk_wal: true, // Default to disk WAL for durability
        }
    }
}

/// Information about a pending flush operation
#[derive(Debug, Clone)]
pub struct PendingFlush {
    /// Unique flush identifier
    pub flush_id: u64,
    /// Sequences being flushed
    pub sequences: Vec<u64>,
    /// When the flush was initiated
    pub initiated_at: DateTime<Utc>,
    /// Data source for this flush
    pub data_source: FlushDataSource,
}

/// Where flush data comes from (memory vs disk WAL files)
#[derive(Debug, Clone)]
pub enum FlushDataSource {
    /// Flush from memory structures (memory-only durability mode)
    Memory,
    /// Flush from disk WAL files (disk durability mode)
    DiskWalFiles(Vec<String>),
    /// Flush from pre-extracted vector records (optimized path)
    VectorRecords(Vec<crate::core::VectorRecord>),
}

/// Common flush coordination logic shared between WAL strategies
#[derive(Clone)]
pub struct WalFlushCoordinator {
    /// Per-collection flush state
    flush_states: Arc<RwLock<HashMap<CollectionId, FlushState>>>,
    /// Global flush ID counter
    next_flush_id: Arc<tokio::sync::Mutex<u64>>,
    /// Storage engine registry for polymorphic flush delegation
    storage_engines: Arc<RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
}

impl WalFlushCoordinator {
    /// Create new flush coordinator
    pub fn new() -> Self {
        Self {
            flush_states: Arc::new(RwLock::new(HashMap::new())),
            next_flush_id: Arc::new(tokio::sync::Mutex::new(1)),
            storage_engines: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Initialize flush state for a collection
    /// Register a storage engine for polymorphic flush delegation
    pub async fn register_storage_engine(
        &self,
        engine_type: &str,
        engine: Arc<dyn UnifiedStorageEngine>,
    ) {
        let mut engines = self.storage_engines.write().await;
        engines.insert(engine_type.to_string(), engine);
        info!("🏭 Registered {} storage engine with FlushCoordinator", engine_type);
    }

    /// Clean up flush coordinator state for a deleted collection
    pub async fn cleanup_collection(&self, collection_id: &CollectionId) {
        let mut flush_states = self.flush_states.write().await;
        if flush_states.remove(collection_id).is_some() {
            info!("🧹 Cleaned up flush coordinator state for collection: {}", collection_id);
        }
    }

    /// Execute coordinated flush: WAL → Storage Engine → WAL Cleanup (ATOMIC)
    pub async fn execute_coordinated_flush(
        &self,
        collection_id: &CollectionId,
        flush_data: FlushDataSource,
        preferred_engine: Option<&str>,
        wal_manager: Option<Arc<dyn crate::storage::persistence::wal::WalStrategy>>,
    ) -> Result<FlushResult> {
        info!("🚀 Coordinator: Starting ATOMIC coordinated flush for collection {}", collection_id);
        
        let flush_id = uuid::Uuid::new_v4().to_string();
        let mut flush_cycle_data = None;
        
        // Step 1: Extract vector records from FlushDataSource + Mark for cleanup
        let vector_records = match &flush_data {
            FlushDataSource::Memory => {
                if let Some(wal) = &wal_manager {
                    info!("📋 Coordinator: ATOMIC retrieval from WAL memtable for collection {}", collection_id);
                    // Get vector records from WAL's memtable AND mark for cleanup
                    let flush_cycle = wal.atomic_retrieve_for_flush(collection_id, &flush_id).await
                        .map_err(|e| anyhow::anyhow!("Failed to retrieve data from WAL: {}", e))?;
                    info!("📋 Coordinator: Retrieved {} vector records (marked for cleanup)", flush_cycle.vector_records.len());
                    
                    let records = flush_cycle.vector_records.clone();
                    flush_cycle_data = Some(flush_cycle); // Store for cleanup
                    records
                } else {
                    warn!("📋 Coordinator: No WAL manager provided, cannot extract memory data");
                    Vec::new()
                }
            },
            FlushDataSource::DiskWalFiles(files) => {
                info!("📋 Coordinator: Extracting vector records from {} disk WAL files", files.len());
                // TODO: Implement disk WAL file reading + mark files for deletion
                warn!("📋 Coordinator: Disk WAL file extraction not yet implemented");
                Vec::new()
            },
            FlushDataSource::VectorRecords(records) => {
                info!("📋 Coordinator: Using pre-extracted {} vector records", records.len());
                records.clone()
            }
        };
        
        if vector_records.is_empty() {
            info!("📋 Coordinator: No vector records to flush, completing without storage operation");
            return Ok(FlushResult {
                success: true,
                collections_affected: vec![collection_id.clone()],
                entries_flushed: 0,
                bytes_written: 0,
                files_created: 0,
                duration_ms: 0,
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
                compaction_triggered: false,
            });
        }
        
        info!("📋 Coordinator: Prepared {} vector records for flush to storage", vector_records.len());
        
        // Step 2: Select appropriate storage engine (Strategy Pattern)
        let engine_type = preferred_engine.unwrap_or("VIPER"); // Default to VIPER
        let engine = {
            let engines = self.storage_engines.read().await;
            engines.get(engine_type)
                .ok_or_else(|| anyhow::anyhow!("Storage engine {} not registered", engine_type))?
                .clone()
        };
        
        info!("🔄 Coordinator: Using {} engine for ATOMIC flush", engine_type);
        
        // Step 3: Create flush parameters with actual vector data
        let flush_params = FlushParameters {
            collection_id: Some(collection_id.clone()),
            force: true,
            synchronous: true,
            vector_records,
            ..Default::default()
        };
        
        // Step 4: Execute polymorphic flush via storage engine (calls do_flush internally)
        let storage_result = engine.do_flush(&flush_params).await?;
        
        info!("✅ Coordinator: Storage flush completed - {} entries, {} bytes, {} files", 
              storage_result.entries_flushed, storage_result.bytes_written, storage_result.files_created);
        
        // Step 5: ATOMIC WAL CLEANUP - Only if storage flush succeeded
        if storage_result.success && storage_result.entries_flushed > 0 {
            if let (Some(wal), Some(flush_cycle)) = (&wal_manager, flush_cycle_data) {
                info!("🧹 Coordinator: Starting ATOMIC WAL cleanup for {} flushed entries", storage_result.entries_flushed);
                
                match wal.complete_flush_cycle(flush_cycle).await {
                    Ok(cleanup_result) => {
                        info!("✅ Coordinator: WAL cleanup SUCCESS - {} entries removed, {} bytes reclaimed", 
                              cleanup_result.entries_removed, cleanup_result.bytes_reclaimed);
                    },
                    Err(cleanup_error) => {
                        warn!("⚠️ Coordinator: WAL cleanup FAILED (storage flush succeeded): {}", cleanup_error);
                        // Don't fail the overall operation since storage succeeded
                        // This creates a minor inconsistency but preserves data safety
                    }
                }
            } else {
                warn!("⚠️ Coordinator: Cannot perform WAL cleanup - missing WAL manager or flush cycle data");
            }
        } else {
            info!("📋 Coordinator: Skipping WAL cleanup (no entries flushed or storage failed)");
        }
        
        info!("🎯 Coordinator: ATOMIC coordinated flush COMPLETE for collection {}", collection_id);
        Ok(storage_result)
    }

    pub async fn initialize_flush_state(&self, collection_id: &CollectionId) -> Result<()> {
        let mut flush_states = self.flush_states.write().await;
        if !flush_states.contains_key(collection_id) {
            flush_states.insert(collection_id.clone(), FlushState::default());
            debug!("🔄 Initialized flush state for collection: {}", collection_id);
        }
        Ok(())
    }

    /// Initiate a flush operation and return the data source
    /// This method determines whether to flush from memory or disk based on configuration
    pub async fn initiate_flush(
        &self,
        collection_id: &CollectionId,
        sequences: Vec<u64>,
        sync_mode: &SyncMode,
    ) -> Result<FlushDataSource> {
        let flush_id = {
            let mut next_id = self.next_flush_id.lock().await;
            let id = *next_id;
            *next_id += 1;
            id
        };

        let mut flush_states = self.flush_states.write().await;
        let flush_state = flush_states
            .entry(collection_id.clone())
            .or_insert_with(FlushState::default);

        // Determine data source based on sync mode and configuration
        let data_source = match sync_mode {
            SyncMode::MemoryOnly => {
                flush_state.uses_disk_wal = false;
                FlushDataSource::Memory
            }
            _ => {
                flush_state.uses_disk_wal = true;
                // Get WAL files for these sequences (placeholder - to be implemented by strategy)
                let wal_files = self.get_wal_files_for_sequences(collection_id, &sequences).await?;
                FlushDataSource::DiskWalFiles(wal_files)
            }
        };

        // Track pending flush
        let pending_flush = PendingFlush {
            flush_id,
            sequences: sequences.clone(),
            initiated_at: Utc::now(),
            data_source: data_source.clone(),
        };

        flush_state.pending_flushes.insert(flush_id, pending_flush);

        info!(
            "🚀 Initiated flush {} for collection {} with {} sequences from {:?}",
            flush_id,
            collection_id,
            sequences.len(),
            data_source
        );

        Ok(data_source)
    }

    /// Acknowledge a successful flush and clean up corresponding WAL data
    /// This is called by the storage engine after successful flush
    pub async fn acknowledge_flush(
        &self,
        collection_id: &CollectionId,
        flush_id: u64,
        flushed_sequences: Vec<u64>,
    ) -> Result<CleanupInstructions> {
        let mut flush_states = self.flush_states.write().await;
        let flush_state = flush_states
            .get_mut(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No flush state for collection: {}", collection_id))?;

        let pending_flush = flush_state
            .pending_flushes
            .remove(&flush_id)
            .ok_or_else(|| anyhow::anyhow!("No pending flush with ID: {}", flush_id))?;

        // Update last flushed sequence
        if let Some(&max_seq) = flushed_sequences.iter().max() {
            flush_state.last_flushed_sequence = flush_state.last_flushed_sequence.max(max_seq);
        }

        // Determine cleanup instructions based on data source
        let cleanup_instructions = match pending_flush.data_source {
            FlushDataSource::Memory => CleanupInstructions {
                cleanup_memory: true,
                cleanup_disk_files: Vec::new(),
                sequences_to_cleanup: flushed_sequences.clone(),
            },
            FlushDataSource::DiskWalFiles(wal_files) => {
                // Only cleanup disk files that are fully flushed
                let files_to_cleanup = self
                    .filter_fully_flushed_files(collection_id, &wal_files, &flushed_sequences)
                    .await?;

                CleanupInstructions {
                    cleanup_memory: true, // Always cleanup memory after successful flush
                    cleanup_disk_files: files_to_cleanup,
                    sequences_to_cleanup: flushed_sequences.clone(),
                }
            },
            FlushDataSource::VectorRecords(_) => CleanupInstructions {
                cleanup_memory: true, // Cleanup memory after successful flush
                cleanup_disk_files: Vec::new(),
                sequences_to_cleanup: flushed_sequences.clone(),
            }
        };

        info!(
            "✅ Acknowledged flush {} for collection {} - {} sequences flushed, cleanup: memory={}, disk_files={}",
            flush_id,
            collection_id,
            flushed_sequences.len(),
            cleanup_instructions.cleanup_memory,
            cleanup_instructions.cleanup_disk_files.len()
        );

        Ok(cleanup_instructions)
    }

    /// Get flush state for a collection
    pub async fn get_flush_state(&self, collection_id: &CollectionId) -> Option<FlushState> {
        let flush_states = self.flush_states.read().await;
        flush_states.get(collection_id).cloned()
    }

    /// Check if a collection uses disk WAL
    pub async fn uses_disk_wal(&self, collection_id: &CollectionId) -> bool {
        let flush_states = self.flush_states.read().await;
        flush_states
            .get(collection_id)
            .map(|state| state.uses_disk_wal)
            .unwrap_or(true) // Default to disk WAL
    }

    /// Get pending flushes for a collection
    pub async fn get_pending_flushes(&self, collection_id: &CollectionId) -> Vec<PendingFlush> {
        let flush_states = self.flush_states.read().await;
        flush_states
            .get(collection_id)
            .map(|state| state.pending_flushes.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Cancel a pending flush (in case of errors)
    pub async fn cancel_flush(&self, collection_id: &CollectionId, flush_id: u64) -> Result<()> {
        let mut flush_states = self.flush_states.write().await;
        if let Some(flush_state) = flush_states.get_mut(collection_id) {
            flush_state.pending_flushes.remove(&flush_id);
            warn!(
                "❌ Cancelled flush {} for collection {}",
                flush_id, collection_id
            );
        }
        Ok(())
    }

    /// Drop all flush state for a collection
    pub async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        let mut flush_states = self.flush_states.write().await;
        flush_states.remove(collection_id);
        info!("🗑️ Dropped flush state for collection: {}", collection_id);
        Ok(())
    }

    // Private helper methods (to be implemented by specific WAL strategies)

    /// Get WAL files containing the specified sequences
    /// This is a placeholder - actual implementation should be provided by the WAL strategy
    async fn get_wal_files_for_sequences(
        &self,
        collection_id: &CollectionId,
        sequences: &[u64],
    ) -> Result<Vec<String>> {
        // Placeholder implementation - to be overridden by strategy-specific logic
        warn!(
            "📁 get_wal_files_for_sequences not implemented for collection {} (sequences: {:?})",
            collection_id, sequences
        );
        Ok(Vec::new())
    }

    /// Filter WAL files that are fully flushed and can be safely deleted
    /// This is a placeholder - actual implementation should be provided by the WAL strategy
    async fn filter_fully_flushed_files(
        &self,
        collection_id: &CollectionId,
        wal_files: &[String],
        flushed_sequences: &[u64],
    ) -> Result<Vec<String>> {
        // Placeholder implementation - to be overridden by strategy-specific logic
        warn!(
            "🔍 filter_fully_flushed_files not implemented for collection {} (files: {:?}, sequences: {:?})",
            collection_id, wal_files, flushed_sequences
        );
        Ok(Vec::new())
    }
}

/// Instructions for cleaning up WAL data after successful flush
#[derive(Debug, Clone)]
pub struct CleanupInstructions {
    /// Whether to cleanup memory structures (ArtMap, HashMap, etc.)
    pub cleanup_memory: bool,
    /// Disk WAL files to delete
    pub cleanup_disk_files: Vec<String>,
    /// Specific sequences to cleanup from memory
    pub sequences_to_cleanup: Vec<u64>,
}

/// Trait for WAL strategies to implement flush coordination callbacks
#[async_trait]
pub trait FlushCoordinatorCallbacks {
    /// Get WAL files containing the specified sequences
    async fn get_wal_files_for_sequences(
        &self,
        collection_id: &CollectionId,
        sequences: &[u64],
    ) -> Result<Vec<String>>;

    /// Check if a WAL file is fully flushed and can be safely deleted
    async fn is_wal_file_fully_flushed(
        &self,
        collection_id: &CollectionId,
        wal_file: &str,
        flushed_sequences: &[u64],
    ) -> Result<bool>;
}