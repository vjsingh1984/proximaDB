//! Atomic Batch Operations for WAL
//!
//! This module implements atomic batch operations that ensure ACID properties
//! for WAL writes, addressing the "half-write" problem where memtable writes
//! succeed but disk writes fail.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::time::Duration;

use crate::storage::atomicity::{
    AtomicOperation, OperationPriority, OperationResult, OperationType, ResourceId,
    TransactionContext,
};
use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
use crate::storage::persistence::wal::{WalDiskManager, WalEntry};

/// Atomic WAL batch operation that ensures memtable and disk consistency
#[derive(Debug)]
pub struct AtomicWalBatchOperation {
    /// WAL entries to write
    pub entries: Vec<WalEntry>,
    /// Deserialized vector batch for memtable
    pub vector_batch: WalVectorBatch,
    /// Collection ID for the operation
    pub collection_id: String,
    /// Whether to perform immediate disk sync
    pub immediate_sync: bool,
    /// Memtable reference for operations
    pub memtable: crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper,
    /// Disk manager reference for operations
    pub disk_manager: Option<std::sync::Arc<WalDiskManager>>,
    /// Rollback state tracking
    rollback_state: std::sync::Arc<tokio::sync::Mutex<Option<AtomicBatchRollbackState>>>,
}

/// State needed for rollback operations
#[derive(Debug)]
struct AtomicBatchRollbackState {
    /// Whether memtable write was successful
    memtable_written: bool,
    /// Whether disk write was successful  
    disk_written: bool,
    /// Batch ID for rollback operations
    batch_id: String,
    /// Sequences returned by memtable write
    sequences: Option<Vec<u64>>,
}

impl AtomicWalBatchOperation {
    /// Create new atomic batch operation
    pub fn new(
        entries: Vec<WalEntry>,
        vector_batch: WalVectorBatch,
        collection_id: String,
        immediate_sync: bool,
        memtable: crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper,
        disk_manager: Option<std::sync::Arc<WalDiskManager>>,
    ) -> Self {
        Self {
            entries,
            vector_batch,
            collection_id,
            immediate_sync,
            memtable,
            disk_manager,
            rollback_state: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

#[async_trait]
impl AtomicOperation for AtomicWalBatchOperation {
    /// Execute the atomic batch operation (memtable + disk)
    async fn execute(&self, _context: &mut TransactionContext) -> Result<OperationResult> {
        let start_time = std::time::Instant::now();
        let batch_id = self.vector_batch.batch_id.batch_uuid.clone();
        
        tracing::info!(
            "🔒 ATOMIC_BATCH: Starting atomic batch operation for {} entries (batch_id: {})",
            self.entries.len(),
            batch_id
        );

        // Initialize rollback state
        *self.rollback_state.lock().await = Some(AtomicBatchRollbackState {
            memtable_written: false,
            disk_written: false,
            batch_id: batch_id.clone(),
            sequences: None,
        });

        // Phase 1: Write to memtable
        let memtable_start = std::time::Instant::now();
        let sequences = self
            .memtable
            .add_vector_batch(self.vector_batch.clone())
            .await
            .context("Failed to write batch to memtable")?;
        let memtable_time = memtable_start.elapsed().as_micros();

        // Update rollback state
        if let Some(ref mut rollback_state) = self.rollback_state.lock().await.as_mut() {
            rollback_state.memtable_written = true;
            rollback_state.sequences = Some(sequences.clone());
        }

        tracing::debug!(
            "🔒 ATOMIC_BATCH: Phase 1 complete - memtable write succeeded ({}μs, batch_id: {})",
            memtable_time,
            batch_id
        );

        // Phase 2: Write to disk (if immediate_sync is required)
        let mut disk_time = 0u128;
        if self.immediate_sync {
            if let Some(disk_manager) = &self.disk_manager {
                let disk_start = std::time::Instant::now();

                // Serialize entries for disk storage
                let serialized_data = self.serialize_entries_for_disk().await
                    .context("Failed to serialize entries for disk")?;

                match disk_manager.write_raw(&self.collection_id, serialized_data).await {
                    Ok(_flush_result) => {
                        disk_time = disk_start.elapsed().as_micros();
                        
                        // Update rollback state
                        if let Some(ref mut rollback_state) = self.rollback_state.lock().await.as_mut() {
                            rollback_state.disk_written = true;
                        }

                        tracing::debug!(
                            "🔒 ATOMIC_BATCH: Phase 2 complete - disk write succeeded ({}μs, batch_id: {})",
                            disk_time,
                            batch_id
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "🔒 ATOMIC_BATCH: Phase 2 failed - disk write error (batch_id: {}): {}",
                            batch_id,
                            e
                        );
                        
                        // Disk write failed - this will trigger rollback
                        return Err(anyhow::anyhow!(
                            "Atomic batch operation failed: disk write error for batch {}: {}",
                            batch_id,
                            e
                        ));
                    }
                }
            } else {
                return Err(anyhow::anyhow!(
                    "Atomic batch operation failed: immediate_sync requested but no disk manager available"
                ));
            }
        }

        let total_time = start_time.elapsed().as_micros();

        tracing::info!(
            "✅ ATOMIC_BATCH: Operation complete - batch_id: {}, memtable: {}μs, disk: {}μs, total: {}μs",
            batch_id,
            memtable_time,
            disk_time,
            total_time
        );

        // Return operation result
        Ok(OperationResult::BatchWrite {
            batch_id,
            sequences_written: sequences.len() as u64,
            bytes_written: self.vector_batch.total_size_bytes as u64,
            memtable_time_us: memtable_time,
            disk_time_us: disk_time,
            total_time_us: total_time,
        })
    }

    /// Rollback the operation if transaction fails
    async fn rollback(&self, _context: &TransactionContext) -> Result<()> {
        if let Some(rollback_state) = &*self.rollback_state.lock().await {
            tracing::warn!(
                "🔄 ATOMIC_BATCH: Rolling back batch operation (batch_id: {})",
                rollback_state.batch_id
            );

            // If memtable write succeeded, we need to remove the batch
            if rollback_state.memtable_written {
                tracing::info!(
                    "🔄 ATOMIC_BATCH: Removing batch from memtable (batch_id: {})",
                    rollback_state.batch_id
                );

                // Use the batch-specific rollback method
                if let Err(e) = self
                    .memtable
                    .remove_batch(&self.collection_id, &rollback_state.batch_id)
                    .await
                {
                    tracing::error!(
                        "❌ ATOMIC_BATCH: Failed to rollback memtable batch {}: {}",
                        rollback_state.batch_id,
                        e
                    );
                    return Err(anyhow::anyhow!(
                        "Failed to rollback memtable batch {}: {}",
                        rollback_state.batch_id,
                        e
                    ));
                }

                tracing::info!(
                    "✅ ATOMIC_BATCH: Successfully rolled back batch from memtable (batch_id: {})",
                    rollback_state.batch_id
                );
            }

            // Note: We don't need to rollback disk writes since they're append-only
            // and failed disk writes don't leave partial state
        }

        Ok(())
    }

    /// Validate operation before execution
    async fn validate(&self, _context: &TransactionContext) -> Result<()> {
        // Validate entries are not empty
        if self.entries.is_empty() {
            return Err(anyhow::anyhow!("Cannot execute empty batch operation"));
        }

        // Validate collection consistency
        let collection_id = &self.entries[0].collection_id;
        for entry in &self.entries {
            if entry.collection_id != *collection_id {
                return Err(anyhow::anyhow!(
                    "Batch contains entries from multiple collections"
                ));
            }
        }

        // Validate vector batch consistency
        if self.vector_batch.batch_id.collection_id != self.collection_id {
            return Err(anyhow::anyhow!(
                "Vector batch collection_id does not match operation collection_id"
            ));
        }

        // Validate disk manager availability if immediate sync required
        if self.immediate_sync && self.disk_manager.is_none() {
            return Err(anyhow::anyhow!(
                "Immediate sync requested but no disk manager available"
            ));
        }

        Ok(())
    }

    /// Get operation type for logging and metrics
    fn operation_type(&self) -> OperationType {
        OperationType::BatchWrite
    }

    /// Get affected resources for conflict detection
    fn affected_resources(&self) -> Vec<ResourceId> {
        vec![
            ResourceId::Collection(self.collection_id.clone()),
            ResourceId::MemtablePartition(self.collection_id.clone()),
        ]
    }

    /// Get operation priority for scheduling
    fn priority(&self) -> OperationPriority {
        // Batch writes are high priority to maintain throughput
        OperationPriority::High
    }

    /// Estimate operation duration for timeout management
    fn estimated_duration(&self) -> Duration {
        // Base time + per-entry overhead + disk sync overhead
        let base_time = Duration::from_millis(10);
        let per_entry_time = Duration::from_micros(100);
        let disk_sync_time = if self.immediate_sync {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(0)
        };

        base_time + (per_entry_time * self.entries.len() as u32) + disk_sync_time
    }
}

impl AtomicWalBatchOperation {
    /// Serialize entries for disk storage (placeholder implementation)
    async fn serialize_entries_for_disk(&self) -> Result<Vec<u8>> {
        // This would use the appropriate serialization based on the WAL strategy (Avro/Bincode)
        // For now, use a simple serialization
        Ok(bincode::serialize(&self.entries)
            .context("Failed to serialize entries for disk")?)
    }
}

