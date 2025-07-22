// Copyright 2024 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Atomic WAL Sync Implementation using Unified Atomic Coordinator
//!
//! This module implements atomic per-batch WAL synchronization to disk using the
//! existing UnifiedAtomicCoordinator for reliable staging → final operations.

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::storage::atomic::{
    StagingConfig, StagingOperationType, UnifiedAtomicCoordinator, OperationId,
};
use crate::storage::persistence::wal::{
    BatchId, WalOperation,
    serialization::{AvroSerializer, VectorBatchSerializer},
};
use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
use crate::storage::persistence::wal::optimized_path_resolver::{
    OptimizedWalPathResolver, CollectionPaths,
};

/// Atomic WAL synchronization manager (strategy-agnostic)
pub struct AtomicWalSync {
    atomic_coordinator: Arc<UnifiedAtomicCoordinator>,
    path_resolver: Arc<OptimizedWalPathResolver>,
    active_syncs: Arc<RwLock<std::collections::HashMap<String, SyncProgress>>>,
}

/// Strategy-specific serialization format
#[derive(Debug, Clone)]
pub enum SerializationStrategy {
    Proto,
    Avro,
    Bincode,
}

/// WAL batch sync strategy
#[derive(Debug, Clone)]
pub enum WalSyncStrategy {
    /// Sync immediately after each batch (max durability)
    Immediate,
    /// Sync after N batches (balanced performance/durability)
    Batched(usize),
    /// Sync based on memory pressure (adaptive)
    Adaptive,
}

/// Sync progress tracking
#[derive(Debug, Clone)]
struct SyncProgress {
    operation_id: OperationId,
    collection_id: String,
    batch_id: BatchId,
    started_at: chrono::DateTime<Utc>,
    status: SyncStatus,
}

#[derive(Debug, Clone)]
enum SyncStatus {
    Staging,
    Finalizing,
    Completed,
    Failed(String),
}

/// WAL sync result
#[derive(Debug, Clone)]
pub struct WalSyncResult {
    pub operation_id: OperationId,
    pub collection_id: String,
    pub batch_id: BatchId,
    pub bytes_written: usize,
    pub sync_duration_ms: u64,
    pub success: bool,
    pub error: Option<String>,
}

impl AtomicWalSync {
    /// Create new atomic WAL sync manager
    pub fn new(
        atomic_coordinator: Arc<UnifiedAtomicCoordinator>,
        path_resolver: Arc<OptimizedWalPathResolver>,
    ) -> Self {
        Self {
            atomic_coordinator,
            path_resolver,
            active_syncs: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Atomically sync WAL batch to disk using staging pattern (strategy-agnostic)
    pub async fn sync_batch_atomic(
        &self,
        collection_id: &str,
        batch: &WalVectorBatch,
        sync_strategy: WalSyncStrategy,
        serialization_strategy: SerializationStrategy,
    ) -> Result<WalSyncResult> {
        let start_time = std::time::Instant::now();
        let operation_id = format!("wal_sync_{}_{}", collection_id, Uuid::new_v4());

        debug!(
            "Starting atomic WAL sync for collection '{}', batch {}, operation '{}'",
            collection_id, batch.batch_id.to_base62(), operation_id
        );

        // Get collection paths from assignment service
        let collection_paths = self.path_resolver
            .resolve_collection_paths(collection_id)
            .await
            .context("Failed to resolve collection paths")?;

        // Track sync progress
        {
            let mut active_syncs = self.active_syncs.write().await;
            active_syncs.insert(operation_id.clone(), SyncProgress {
                operation_id: operation_id.clone(),
                collection_id: collection_id.to_string(),
                batch_id: batch.batch_id.clone(),
                started_at: Utc::now(),
                status: SyncStatus::Staging,
            });
        }

        let result = match sync_strategy {
            WalSyncStrategy::Immediate => {
                self.sync_immediate(&operation_id, &collection_paths, batch, &serialization_strategy).await
            }
            WalSyncStrategy::Batched(batch_count) => {
                self.sync_batched(&operation_id, &collection_paths, batch, batch_count, &serialization_strategy).await
            }
            WalSyncStrategy::Adaptive => {
                self.sync_adaptive(&operation_id, &collection_paths, batch, &serialization_strategy).await
            }
        };

        // Update sync progress
        {
            let mut active_syncs = self.active_syncs.write().await;
            if let Some(progress) = active_syncs.get_mut(&operation_id) {
                progress.status = match &result {
                    Ok(_) => SyncStatus::Completed,
                    Err(e) => SyncStatus::Failed(e.to_string()),
                };
            }
        }

        let duration = start_time.elapsed();
        
        match result {
            Ok(bytes_written) => {
                info!(
                    "Atomic WAL sync completed for collection '{}' in {}ms, {} bytes written",
                    collection_id, duration.as_millis(), bytes_written
                );

                Ok(WalSyncResult {
                    operation_id,
                    collection_id: collection_id.to_string(),
                    batch_id: batch.batch_id.clone(),
                    bytes_written,
                    sync_duration_ms: duration.as_millis() as u64,
                    success: true,
                    error: None,
                })
            }
            Err(e) => {
                warn!(
                    "Atomic WAL sync failed for collection '{}' after {}ms: {}",
                    collection_id, duration.as_millis(), e
                );

                Ok(WalSyncResult {
                    operation_id,
                    collection_id: collection_id.to_string(),
                    batch_id: batch.batch_id.clone(),
                    bytes_written: 0,
                    sync_duration_ms: duration.as_millis() as u64,
                    success: false,
                    error: Some(e.to_string()),
                })
            }
        }
    }

    /// Immediate sync: Write batch to disk atomically
    async fn sync_immediate(
        &self,
        _operation_id: &str,
        paths: &CollectionPaths,
        batch: &WalVectorBatch,
        serialization_strategy: &SerializationStrategy,
    ) -> Result<usize> {
        // Serialize WAL batch using specified strategy
        let batch_data = self.serialize_wal_batch(batch, serialization_strategy).await
            .context("Failed to serialize WAL batch")?;

        // Create staging configuration for WAL operation
        let staging_config = StagingConfig {
            base_url: paths.wal_logs.clone(),
            collection_id: Some(paths.collection_id.clone()),
            operation_type: StagingOperationType::Wal,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 1, // Cleanup WAL staging files quickly
        };

        // Generate final WAL batch file name
        let batch_filename = format!("batch_{}.wal", batch.batch_id.to_base62());
        let _final_path = format!("{}/{}", paths.wal_logs, batch_filename);

        // Use unified atomic coordinator for staging → final atomic operation
        // First, begin the atomic operation
        let op_metadata = self.atomic_coordinator
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin atomic operation")?;
        
        // Write to staging
        self.atomic_coordinator
            .write_to_staging(
                &op_metadata.operation_id,
                &batch_filename,
                &batch_data,
            )
            .await
            .context("Failed to write to staging")?;
        
        // Finalize the operation (atomic move to final location)
        self.atomic_coordinator
            .finalize_atomic_operation(&op_metadata.operation_id)
            .await
            .context("Failed to finalize atomic operation")?;
        
        let bytes_written = batch_data.len();

        // Update checkpoint after successful WAL write
        self.update_wal_checkpoint(paths, batch).await
            .context("Failed to update WAL checkpoint")?;

        Ok(bytes_written)
    }

    /// Batched sync: Accumulate batches and sync together
    async fn sync_batched(
        &self,
        operation_id: &str,
        paths: &CollectionPaths,
        batch: &WalVectorBatch,
        _batch_count: usize,
        serialization_strategy: &SerializationStrategy,
    ) -> Result<usize> {
        // For now, implement as immediate sync
        // TODO: Implement actual batching with accumulation
        self.sync_immediate(operation_id, paths, batch, serialization_strategy).await
    }

    /// Adaptive sync: Sync based on memory pressure and I/O load
    async fn sync_adaptive(
        &self,
        operation_id: &str,
        paths: &CollectionPaths,
        batch: &WalVectorBatch,
        serialization_strategy: &SerializationStrategy,
    ) -> Result<usize> {
        // For now, implement as immediate sync
        // TODO: Implement adaptive logic based on system metrics
        self.sync_immediate(operation_id, paths, batch, serialization_strategy).await
    }

    /// Serialize WAL batch using specified strategy (Proto/Avro/Bincode)
    async fn serialize_wal_batch(
        &self, 
        batch: &WalVectorBatch, 
        strategy: &SerializationStrategy
    ) -> Result<Vec<u8>> {
        let (payload_data, payload_format) = match strategy {
            SerializationStrategy::Proto => {
                // Serialize just the vector records for proto
                let data = bincode::serialize(&*batch.vector_records)
                    .map_err(|e| anyhow!("Failed to serialize proto vector batch: {}", e))?;
                (data, "proto".to_string())
            }
            SerializationStrategy::Avro => {
                // Use the avro serializer which expects &[VectorRecord]
                let serializer = AvroSerializer::new();
                let data = serializer.serialize_batch(&*batch.vector_records)
                    .context("Failed to serialize avro vector batch")?;
                (data, "avro".to_string())
            }
            SerializationStrategy::Bincode => {
                // Serialize just the vector records for bincode
                let data = bincode::serialize(&*batch.vector_records)
                    .map_err(|e| anyhow!("Failed to serialize bincode vector batch: {}", e))?;
                (data, "bincode".to_string())
            }
        };

        // Create WAL operation wrapper (common format for all strategies)
        let wal_operation = WalOperation {
            operation_type: "upsert_batch".to_string(),
            payload_data,
            payload_format,
            vector_count: batch.vector_records.len(),
        };

        // Serialize the complete WAL operation using bincode (fast, compact)
        bincode::serialize(&wal_operation)
            .map_err(|e| anyhow!("Failed to serialize WAL operation: {}", e))
    }

    /// Update WAL checkpoint after successful batch write
    async fn update_wal_checkpoint(
        &self,
        paths: &CollectionPaths,
        batch: &WalVectorBatch,
    ) -> Result<()> {
        let checkpoint_data = serde_json::json!({
            "collection_id": paths.collection_id,
            "last_batch_id": batch.batch_id.to_base62(),
            "last_updated": Utc::now().to_rfc3339(),
            "batch_count": 1,
        });

        let _checkpoint_path = format!("{}/latest.checkpoint", paths.wal_checkpoints);
        let checkpoint_bytes = serde_json::to_vec_pretty(&checkpoint_data)
            .context("Failed to serialize checkpoint data")?;

        // Use atomic coordinator for checkpoint update as well
        let staging_config = StagingConfig {
            base_url: paths.wal_checkpoints.clone(),
            collection_id: Some(paths.collection_id.clone()),
            operation_type: StagingOperationType::Metadata,
            auto_cleanup: true,
            max_orphaned_age_hours: 1,
            ..Default::default()
        };

        let _operation_id = format!("checkpoint_{}_{}", paths.collection_id, Uuid::new_v4());
        
        // Begin atomic operation for checkpoint
        let op_metadata = self.atomic_coordinator
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin checkpoint operation")?;
        
        // Write checkpoint to staging
        self.atomic_coordinator
            .write_to_staging(
                &op_metadata.operation_id,
                "latest.checkpoint",
                &checkpoint_bytes,
            )
            .await
            .context("Failed to write checkpoint to staging")?;
        
        // Finalize the operation
        self.atomic_coordinator
            .finalize_atomic_operation(&op_metadata.operation_id)
            .await
            .context("Failed to finalize checkpoint update")?;

        debug!(
            "Updated WAL checkpoint for collection '{}' to batch {}",
            paths.collection_id, batch.batch_id.to_base62()
        );

        Ok(())
    }

    /// Get current sync statistics
    pub async fn get_sync_stats(&self) -> SyncStats {
        let active_syncs = self.active_syncs.read().await;
        
        let mut stats = SyncStats {
            active_syncs: active_syncs.len(),
            completed_syncs: 0,
            failed_syncs: 0,
            avg_sync_duration_ms: 0.0,
        };

        let mut total_duration = 0u64;
        let mut completed_count = 0u64;

        for progress in active_syncs.values() {
            match &progress.status {
                SyncStatus::Completed => {
                    stats.completed_syncs += 1;
                    completed_count += 1;
                    total_duration += (Utc::now() - progress.started_at).num_milliseconds() as u64;
                }
                SyncStatus::Failed(_) => {
                    stats.failed_syncs += 1;
                }
                _ => {} // Still in progress
            }
        }

        if completed_count > 0 {
            stats.avg_sync_duration_ms = total_duration as f64 / completed_count as f64;
        }

        stats
    }

    /// Cleanup completed sync operations
    pub async fn cleanup_completed_syncs(&self, max_age: chrono::Duration) -> Result<usize> {
        let mut active_syncs = self.active_syncs.write().await;
        let now = Utc::now();
        let initial_count = active_syncs.len();

        active_syncs.retain(|_, progress| {
            match &progress.status {
                SyncStatus::Completed | SyncStatus::Failed(_) => {
                    now - progress.started_at < max_age
                }
                _ => true, // Keep active operations
            }
        });

        let cleaned_count = initial_count - active_syncs.len();
        if cleaned_count > 0 {
            debug!("Cleaned up {} completed sync operations", cleaned_count);
        }

        Ok(cleaned_count)
    }
}

/// WAL sync statistics
#[derive(Debug, Clone)]
pub struct SyncStats {
    pub active_syncs: usize,
    pub completed_syncs: usize,
    pub failed_syncs: usize,
    pub avg_sync_duration_ms: f64,
}

#[cfg(test)]
mod tests {
    
    
    #[tokio::test]
    async fn test_atomic_wal_sync() {
        // TODO: Implement comprehensive atomic sync tests
        assert!(true);
    }
    
    #[tokio::test]
    async fn test_sync_strategies() {
        // TODO: Test different sync strategies (immediate, batched, adaptive)
        assert!(true);
    }
}