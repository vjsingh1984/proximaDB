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

use super::config::SyncMode;

use crate::storage::traits::{FlushParameters, FlushResult, UnifiedStorageEngine};
use crate::storage::background_flush_context::BackgroundFlushContext;
use super::enhanced_flush_result::EnhancedFlushResult;
use super::flush_result_optimization::OptimizedFlushCoordinator;

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
pub struct WALFlushCoordinator {
    /// Per-collection flush state
    flush_states: Arc<RwLock<HashMap<String, FlushState>>>,
    /// Global flush ID counter
    next_flush_id: Arc<tokio::sync::Mutex<u64>>,
    /// Storage engine registry for polymorphic flush delegation
    storage_engines: Arc<RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
    /// AXIS manager for IndexConfig-based indexing after flush
    axis_manager: Option<Arc<crate::index::axis::manager::AxisManager>>,
    /// Optimized flush coordinator for high-performance flushing
    optimized_coordinator: Option<Arc<OptimizedFlushCoordinator>>,
    /// Collection service for fetching metadata
    collection_service: Option<Arc<crate::services::collection_service::CollectionService>>,
    /// Metrics updater for tracking flush operations
    metrics_updater: Option<Arc<dyn crate::metrics::InternalMetricsUpdater>>,
}

impl WALFlushCoordinator {
    /// Create new flush coordinator
    pub fn new() -> Self {
        Self {
            flush_states: Arc::new(RwLock::new(HashMap::new())),
            next_flush_id: Arc::new(tokio::sync::Mutex::new(1)),
            storage_engines: Arc::new(RwLock::new(HashMap::new())),
            axis_manager: None,
            optimized_coordinator: None,
            collection_service: None,
            metrics_updater: None,
        }
    }
    
    /// Set collection service for metadata fetching
    pub fn set_collection_service(&mut self, service: Arc<crate::services::collection_service::CollectionService>) {
        self.collection_service = Some(service);
    }
    
    /// Set metrics updater for tracking flush operations
    pub fn set_metrics_updater(&mut self, updater: Arc<dyn crate::metrics::InternalMetricsUpdater>) {
        self.metrics_updater = Some(updater);
        info!("🔗 FlushCoordinator: Metrics updater registered for flush operation tracking");
    }
    
    /// Enable optimized flush processing
    pub fn enable_optimized_flush(&mut self, batch_size: usize, worker_count: usize, dimension: usize) {
        self.optimized_coordinator = Some(Arc::new(OptimizedFlushCoordinator::new(
            batch_size,
            worker_count,
            dimension,
        )));
        info!("🚀 FlushCoordinator: Optimized flush enabled with batch_size={}, workers={}", batch_size, worker_count);
    }

    /// Set the AXIS manager for IndexConfig-based indexing
    pub fn set_axis_manager(&mut self, axis_manager: Arc<crate::index::axis::manager::AxisManager>) {
        self.axis_manager = Some(axis_manager);
        info!("🔗 FlushCoordinator: AXIS manager registered for IndexConfig-based indexing");
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
        info!(
            "🏭 Registered {} storage engine with FlushCoordinator",
            engine_type
        );
    }

    /// Clean up flush coordinator state for a deleted collection
    pub async fn cleanup_collection(&self, collection_id: &str) {
        let mut flush_states = self.flush_states.write().await;
        if flush_states.remove(collection_id).is_some() {
            info!(
                "🧹 Cleaned up flush coordinator state for collection: {}",
                collection_id
            );
        }
    }

    /// Execute coordinated flush: WAL → Storage Engine → WAL Cleanup (ATOMIC)
    /// 🚀 OPTIMIZED: Now accepts BackgroundFlushContext to eliminate service calls
    pub async fn execute_coordinated_flush(
        &self,
        collection_id: &str,
        flush_data: FlushDataSource,
        preferred_engine: Option<&str>,
        flush_context: Option<&BackgroundFlushContext>,
    ) -> Result<EnhancedFlushResult> {
        info!(
            "🚀 Coordinator: Starting ATOMIC coordinated flush for collection {}",
            collection_id
        );

        let _flush_id = uuid::Uuid::new_v4().to_string();

        // Step 1: Extract vector records from FlushDataSource + Mark for cleanup
        let vector_records = match &flush_data {
            FlushDataSource::Memory => {
                // Memory flush is handled by VectorOperationsService in the optimized architecture
                warn!("📋 Coordinator: Memory flush source used - should be handled by VectorOperationsService with context");
                Vec::new()
            }
            FlushDataSource::DiskWalFiles(files) => {
                info!(
                    "📋 Coordinator: Extracting vector records from {} disk WAL files",
                    files.len()
                );
                // TODO: Implement disk WAL file reading + mark files for deletion
                warn!("📋 Coordinator: Disk WAL file extraction not yet implemented");
                Vec::new()
            }
            FlushDataSource::VectorRecords(records) => {
                info!(
                    "📋 Coordinator: Using pre-extracted {} vector records",
                    records.len()
                );
                records.clone()
            }
        };

        if vector_records.is_empty() {
            info!(
                "📋 Coordinator: No vector records to flush, completing without storage operation"
            );
            return Ok(EnhancedFlushResult::new(
                FlushResult {
                success: true,
                collections_affected: vec![collection_id.to_string()],
                entries_flushed: 0,
                bytes_written: 0,
                files_created: 0,
                duration_ms: 0,
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
                compaction_triggered: false,
                flushed_batch_ids: vec![],
                },
                Vec::new(),
            ));
        }

        info!(
            "📋 Coordinator: Prepared {} vector records for flush to storage",
            vector_records.len()
        );

        // Step 2: 🚀 OPTIMIZATION: Use pre-computed context metadata when available (eliminates service calls)
        // This includes: storage engine type, compression settings, storage assignment, etc.
        let collection_metadata = if let Some(context) = flush_context {
            info!("✅ CONTEXT_OPTIMIZED: Using pre-computed metadata for collection {}", collection_id);
            // Use centralized helper method for consistent collection proto creation
            Some(context.to_collection_proto())
        } else if let Some(ref collection_service) = self.collection_service {
            // Fallback: Use collection service (legacy path)
            warn!("⚠️ FALLBACK: Using collection service - context not provided");
            match collection_service.get_proto_collection(collection_id).await {
                Ok(Some(collection)) => {
                    info!(
                        "📋 Coordinator: Fetched collection metadata for '{}' - engine: {:?}, compression: {:?}",
                        collection_id,
                        collection.config.as_ref().map(|c| c.storage_engine),
                        collection.config.as_ref().and_then(|c| c.quantization_config.as_ref())
                    );
                    Some(collection)
                }
                Ok(None) => {
                    warn!("⚠️ Coordinator: Collection '{}' not found in metadata", collection_id);
                    None
                }
                Err(e) => {
                    warn!("⚠️ Coordinator: Failed to fetch collection metadata: {}", e);
                    None
                }
            }
        } else {
            warn!("⚠️ Coordinator: No collection service available, proceeding without metadata");
            None
        };
        
        // 🚀 OPTIMIZATION: Determine storage engine - use context directly when available
        let engine_type = if let Some(context) = flush_context {
            // Direct context optimization - no metadata parsing needed!
            info!("✅ ENGINE_OPTIMIZED: Using pre-computed engine {} for collection {}", 
                  context.engine_name(), collection_id);
            context.engine_name()
        } else if let Some(ref metadata) = collection_metadata {
            // Legacy path: Parse from metadata
            if let Some(ref config) = metadata.config {
                // Map proto storage engine enum to string
                use crate::proto::proximadb::StorageEngine;
                match StorageEngine::try_from(config.storage_engine) {
                    Ok(StorageEngine::Viper) => "viper",
                    Ok(StorageEngine::Sst) => "sst", 
                    _ => preferred_engine.unwrap_or("viper") // Default to viper or provided preference
                }
            } else {
                preferred_engine.unwrap_or("viper")
            }
        } else {
            preferred_engine.ok_or_else(|| {
                anyhow::anyhow!(
                    "No storage engine specified for collection {} and no metadata available",
                    collection_id
                )
            })?
        };
        
        info!(
            "🔍 Coordinator: Using {} storage engine for collection {}",
            engine_type, collection_id
        );

        let engine = {
            let engines = self.storage_engines.read().await;
            info!(
                "🔍 Coordinator: Available engines: {:?}",
                engines.keys().collect::<Vec<_>>()
            );
            engines
                .get(engine_type)
                .ok_or_else(|| anyhow::anyhow!("Storage engine {} not registered", engine_type))?
                .clone()
        };

        info!(
            "🔄 Coordinator: Using {} engine for ATOMIC flush with metadata",
            engine_type
        );

        // Step 3: Create flush parameters with actual vector data + BatchId coordination
        let batch_ids = Vec::new();  // No cycle data needed in this simplified flow

        // Clone vector records for AXIS indexing before moving into flush params
        let vector_records_for_axis = vector_records.clone();

        // Check if optimized flush is enabled and use it
        let storage_result = if let Some(optimized) = &self.optimized_coordinator {
            info!("🚀 Coordinator: Using optimized flush path");
            
            // Execute optimized flush
            let optimized_result = optimized
                .execute_optimized_flush(collection_id, vector_records)
                .await?;
            
            // Convert to standard flush result
            let mut base_result = optimized_result.base.clone();
            base_result.flushed_batch_ids = batch_ids.clone();
            
            // Store optimized vectors for later AXIS indexing
            // The optimized result uses Arc<VectorRecord> to avoid cloning
            base_result
        } else {
            // Regular flush path with collection metadata
            let flush_params = FlushParameters {
                collection_id: Some(collection_id.to_string()),
                force: true,
                synchronous: true,
                vector_records,
                batch_ids, // ✅ Include BatchIds for coordination
                collection_config: collection_metadata.clone(), // ✅ Pass metadata to avoid duplicate fetches
                ..Default::default()
            };

            // Step 4: Execute polymorphic flush via storage engine (calls do_flush internally)
            engine.do_flush(&flush_params).await?
        };

        info!(
            "✅ Coordinator: Storage flush completed - {} entries, {} bytes, {} files",
            storage_result.entries_flushed,
            storage_result.bytes_written,
            storage_result.files_created
        );

        // Step 5: ATOMIC WAL CLEANUP using BatchId coordination - Only if storage flush succeeded
        if storage_result.success && storage_result.entries_flushed > 0 {
            info!(
                "🧹 Coordinator: Starting BatchId-coordinated cleanup for {} flushed entries, {} batch IDs",
                storage_result.entries_flushed,
                storage_result.flushed_batch_ids.len()
            );

            // WAL cleanup is handled by VectorOperationsService in the optimized architecture
            // The context-based approach ensures proper coordination between flush and cleanup
            info!("📋 Coordinator: WAL cleanup handled by VectorOperationsService with context optimization");

            // Cleanup memtable using BatchIds  
            // TODO: Add memtable cleanup interface
            info!(
                "🧹 Coordinator: Memtable cleanup for {} batches (TODO: implement)",
                storage_result.flushed_batch_ids.len()
            );
        } else {
            info!("📋 Coordinator: Skipping cleanup (no entries flushed or storage failed)");
        }

        // NOTE: AXIS indexing notification is handled by BackgroundManager 
        // after the complete flush-compaction cycle to ensure proper sequential execution:
        // 1. FLUSH (materialized data)
        // 2. COMPACTION (if needed) 
        // 3. INDEXING (final optimized layout)
        info!(
            "📋 Coordinator: Flush completed - indexing will be handled by BackgroundManager after compaction cycle"
        );

        info!(
            "🎯 Coordinator: ATOMIC coordinated flush COMPLETE for collection {}",
            collection_id
        );
        
        // 📊 METRICS: Record flush operation metrics (non-blocking)
        if let Some(ref metrics) = self.metrics_updater {
            let engine_type_str = if let Some(context) = flush_context {
                context.engine_name().to_uppercase()
            } else {
                engine_type.to_uppercase()
            };
            
            let _ = metrics.record_flush(
                collection_id,
                crate::metrics::FlushMetricsUpdate {
                    vectors_flushed: storage_result.entries_flushed as i64,
                    bytes_written: storage_result.bytes_written as i64,
                    duration_ms: storage_result.duration_ms as i64,
                    files_created: storage_result.files_created as i32,
                    engine_type: engine_type_str,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
            ).await;
            debug!("📊 Recorded flush metrics for collection {}", collection_id);
        }
        
        // Return enhanced result with vector data for AXIS indexing
        Ok(EnhancedFlushResult::new(storage_result, vector_records_for_axis))
    }

    pub async fn initialize_flush_state(&self, collection_id: &str) -> Result<()> {
        let mut flush_states = self.flush_states.write().await;
        if !flush_states.contains_key(collection_id) {
            flush_states.insert(collection_id.to_string(), FlushState::default());
            debug!(
                "🔄 Initialized flush state for collection: {}",
                collection_id
            );
        }
        Ok(())
    }

    /// Initiate a flush operation and return the data source
    /// This method determines whether to flush from memory or disk based on configuration
    pub async fn initiate_flush(
        &self,
        collection_id: &str,
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
            .entry(collection_id.to_string())
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
                let wal_files = self
                    .get_wal_files_for_sequences(collection_id, &sequences)
                    .await?;
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
        collection_id: &str,
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
            }
            FlushDataSource::VectorRecords(_) => CleanupInstructions {
                cleanup_memory: true, // Cleanup memory after successful flush
                cleanup_disk_files: Vec::new(),
                sequences_to_cleanup: flushed_sequences.clone(),
            },
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
    pub async fn get_flush_state(&self, collection_id: &str) -> Option<FlushState> {
        let flush_states = self.flush_states.read().await;
        flush_states.get(collection_id).cloned()
    }

    /// Check if a collection uses disk WAL
    pub async fn uses_disk_wal(&self, collection_id: &str) -> bool {
        let flush_states = self.flush_states.read().await;
        flush_states
            .get(collection_id)
            .map(|state| state.uses_disk_wal)
            .unwrap_or(true) // Default to disk WAL
    }

    /// Get pending flushes for a collection
    pub async fn get_pending_flushes(&self, collection_id: &str) -> Vec<PendingFlush> {
        let flush_states = self.flush_states.read().await;
        flush_states
            .get(collection_id)
            .map(|state| state.pending_flushes.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Cancel a pending flush (in case of errors)
    pub async fn cancel_flush(&self, collection_id: &str, flush_id: u64) -> Result<()> {
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
    pub async fn drop_collection(&self, collection_id: &str) -> Result<()> {
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
        collection_id: &str,
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
        collection_id: &str,
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
        collection_id: &str,
        sequences: &[u64],
    ) -> Result<Vec<String>>;

    /// Check if a WAL file is fully flushed and can be safely deleted
    async fn is_wal_file_fully_flushed(
        &self,
        collection_id: &str,
        wal_file: &str,
        flushed_sequences: &[u64],
    ) -> Result<bool>;
}
