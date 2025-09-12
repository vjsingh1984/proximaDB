// MARKED FOR REMOVAL: This file uses optimized_path_resolver which uses assignment_service
// Atomic sync should be handled through collection metadata
/*
// Copyright 2024 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use tracing::{debug, info, warn};

use crate::storage::transaction_coordinator::{
    TransactionCoordinator,
    TransactionalOperation,
    TransactionLog,
    TransactionState,
    write_ahead_log::WriteBufferTransaction,
};
use crate::storage::persistence::write_ahead_log::{
    BatchId, WALOperation,
    serialization::{AvroSerializer, VectorBatchSerializer},
};
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::persistence::write_ahead_log::optimized_path_resolver::{
    OptimizedWalPathResolver, CollectionPaths,
};

/// Atomic WAL synchronization manager (strategy-agnostic)
pub struct AtomicWalSync {
    atomic_coordinator: Arc<TransactionCoordinator>,
    path_resolver: Arc<OptimizedWalPathResolver>,
    active_syncs: Arc<RwLock<std::collections::HashMap<String, SyncProgress>>>,
}

/// Strategy-specific serialization format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SerializationStrategy {
    Avro,
    Bincode,
    Protobuf,
}

/// Sync operation progress tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProgress {
    pub collection_id: String,
    pub batch_id: BatchId,
    pub vectors_synced: usize,
    pub total_vectors: usize,
    pub bytes_written: u64,
    pub started_at: std::time::Instant,
    pub completed: bool,
}

impl AtomicWalSync {
    /// Create new atomic sync manager
    pub fn new(
        atomic_coordinator: Arc<TransactionCoordinator>,
        path_resolver: Arc<OptimizedWalPathResolver>,
    ) -> Self {
        Self {
            atomic_coordinator,
            path_resolver,
            active_syncs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Atomically sync vectors to disk with transactional guarantees
    pub async fn sync_batch(
        &self,
        collection_id: String,
        batch: WALVectorBatch,
        // strategy removed -  SerializationStrategy,
    ) -> Result<BatchId> {
        let batch_id = BatchId::new();
        let transaction_id = crate::utils::uuid::Uuid::new_v4().to_string();
        
        // Record sync progress
        {
            let mut syncs = self.active_syncs.write().await;
            syncs.insert(
                collection_id.clone(),
                SyncProgress {
                    collection_id: collection_id.clone(),
                    batch_id: batch_id.clone(),
                    vectors_synced: 0,
                    total_vectors: batch.vectors.len(),
                    bytes_written: 0,
                    started_at: std::time::Instant::now(),
                    completed: false,
                },
            );
        }

        // Phase 1: Prepare atomic transaction
        let wal_txn = WriteBufferTransaction {
            transaction_id: transaction_id.clone(),
            collection_id: collection_id.clone(),
            batch_id: batch_id.clone(),
            vectors: batch.vectors.clone(),
            sequences: batch.sequences.clone(),
            timestamp: chrono::Utc::now(),
        };

        // Register transaction with coordinator
        self.atomic_coordinator
            .begin_transaction(transaction_id.clone())
            .await?;

        // Phase 2: Resolve paths using assignment service
        let paths = self.path_resolver
            .resolve_collection_paths(&collection_id)
            .await
            .context("Failed to resolve collection paths")?;

        // Phase 3: Serialize based on strategy
        let serialized_data = match strategy {
            SerializationStrategy::Avro => {
                let serializer = AvroSerializer::new();
                serializer.serialize_batch(&batch)?
            }
            SerializationStrategy::Bincode => {
                bincode::serialize(&batch)
                    .context("Bincode serialization failed")?
            }
            SerializationStrategy::Protobuf => {
                // Convert to proto and serialize
                let proto_batch = Self::convert_to_proto(&batch);
                proto_batch.encode_to_vec()
            }
        };

        // Phase 4: Write atomically with transaction log
        let wal_file_path = format!(
            "{}/batch_{}.wal",
            paths.wal_logs,
            batch_id.0
        );

        // Create transaction log entry
        let log_entry = TransactionLog {
            transaction_id: transaction_id.clone(),
            operation: TransactionalOperation::WriteBufferSync {
                collection_id: collection_id.clone(),
                batch_id: batch_id.clone(),
                vector_count: batch.vectors.len(),
            },
            state: TransactionState::InProgress,
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
        };

        // Write with atomic coordinator
        match self.atomic_coordinator
            .execute_atomic(log_entry, async {
                // Write WAL file
                let filesystem = self.path_resolver
                    .filesystem_factory
                    .get_filesystem(&paths.wal_base)?;
                
                filesystem
                    .write_file(&wal_file_path, &serialized_data)
                    .await
                    .context("Failed to write WAL file")?;

                // Update progress
                {
                    let mut syncs = self.active_syncs.write().await;
                    if let Some(progress) = syncs.get_mut(&collection_id) {
                        progress.vectors_synced = batch.vectors.len();
                        progress.bytes_written = serialized_data.len() as u64;
                        progress.completed = true;
                    }
                }

                Ok(())
            })
            .await
        {
            Ok(_) => {
                // Commit transaction
                self.atomic_coordinator
                    .commit_transaction(&transaction_id)
                    .await?;
                
                info!(
                    "✅ Atomic sync complete: {} vectors to {} ({})",
                    batch.vectors.len(),
                    wal_file_path,
                    humansize::format_size(serialized_data.len(), humansize::BINARY)
                );
                
                Ok(batch_id)
            }
            Err(e) => {
                // Rollback on failure
                self.atomic_coordinator
                    .rollback_transaction(&transaction_id)
                    .await?;
                
                warn!("❌ Atomic sync failed, rolled back: {}", e);
                Err(e.into())
            }
        }
    }

    /// Force immediate sync of pending operations
    pub async fn force_sync(&self, collection_id: &str) -> Result<()> {
        debug!("🔄 Force sync requested for collection: {}", collection_id);
        
        // Check if there's an active sync
        let active = {
            let syncs = self.active_syncs.read().await;
            syncs.contains_key(collection_id)
        };

        if active {
            debug!("⏳ Waiting for active sync to complete: {}", collection_id);
            // Wait for completion
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                let syncs = self.active_syncs.read().await;
                if let Some(progress) = syncs.get(key) {
                    if progress.completed {
                        break;
                    }
                } else {
                    break;
                }
            }
        }

        Ok(())
    }

    /// Get sync progress for monitoring
    pub async fn get_progress(&self, collection_id: &str) -> Option<SyncProgress> {
        let syncs = self.active_syncs.read().await;
        syncs.get(key).cloned()
    }

    /// Convert batch to protobuf format
    fn convert_to_proto(batch: &WALVectorBatch) -> Vec<u8> {
        // Placeholder - implement actual proto conversion
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_atomic_sync() {
        // Test will be implemented when assignment service is refactored
    }
}
*/