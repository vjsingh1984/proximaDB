//! Flush + compaction coordinator extracted from `VectorOperationsService`
//! (Phase 2.1 god-object decomposition, slice 2).
//!
//! Owns the WAL→storage durability/maintenance step: drain the WAL for a
//! collection (or all collections) and trigger engine compaction. Holds only the
//! `Arc` handles it operates on; `VectorOperationsService` keeps its public
//! `force_flush_*` surface and delegates here.

use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use tracing::{debug, info};

use crate::proto::proximadb_v1::Collection;
use crate::storage::persistence::write_ahead_log::WriteAheadLogManager;
use crate::storage::traits::{StorageFormatStrategy, UnifiedStorageFormat};

fn flush_owns_compaction_schedule(strategy: StorageFormatStrategy) -> bool {
    matches!(strategy, StorageFormatStrategy::Sst)
}

/// Coordinates WAL flush + engine compaction for one or all collections.
/// Compaction failures are logged but never fail the flush (the durability step
/// already succeeded once the WAL is drained).
///
/// Holds the engine as `dyn UnifiedStorageFormat` so `compact_collection`
/// dispatches to the trait method (the concrete `SstEngine` also has an inherent
/// `compact_collection` with a different signature).
pub(crate) struct FlushCompactionCoordinator {
    wal_manager: Arc<WriteAheadLogManager>,
    storage_engine: Arc<dyn UnifiedStorageFormat>,
    collection_cache: Arc<DashMap<crate::core::stable_id::CollectionObjectId, Arc<Collection>>>,
}

impl FlushCompactionCoordinator {
    pub(crate) fn new(
        wal_manager: Arc<WriteAheadLogManager>,
        storage_engine: Arc<dyn UnifiedStorageFormat>,
        collection_cache: Arc<DashMap<crate::core::stable_id::CollectionObjectId, Arc<Collection>>>,
    ) -> Self {
        Self {
            wal_manager,
            storage_engine,
            collection_cache,
        }
    }

    /// Force-flush every collection's WAL to storage, then compact each
    /// (best-effort; `compact_all` is not on the engine trait).
    pub(crate) async fn force_flush_all(&self) -> Result<()> {
        info!("🔄 Force flushing all collections");

        // Flush the WAL manager
        self.wal_manager.force_flush_all().await?;

        // Trigger compaction in storage engine
        // Note: compact_all is not available in UnifiedStorageFormat trait
        // Instead, we need to compact each collection individually
        let collections: Vec<crate::core::stable_id::CollectionObjectId> = self
            .collection_cache
            .iter()
            .map(|entry| *entry.key())
            .collect();

        for collection_object_id in collections {
            if let Some(collection) = self.collection_cache.get(&collection_object_id) {
                let collection_id = collection_object_id.to_string();
                match self
                    .storage_engine
                    .compact_collection(&collection_id, Some(&**collection))
                    .await
                {
                    Ok(result) => {
                        info!(
                            "✅ Compacted collection {}: {} files processed",
                            collection_id,
                            result.output_files.unwrap_or(0)
                        );
                    }
                    Err(e) => {
                        debug!(
                            "⚠️ Compaction failed for collection {}: {}",
                            collection_id, e
                        );
                        // Continue with other collections
                    }
                }
            }
        }

        debug!("Force flush all completed");
        Ok(())
    }

    /// Flush all pending WAL entries for a specific collection to durable
    /// storage, then compact it (best-effort).
    pub(crate) async fn force_flush_collection(&self, collection_id: &str) -> Result<()> {
        info!("🔄 Force flushing collection: {}", collection_id);

        // Flush the WAL manager for this collection
        self.wal_manager
            .force_flush_collection(collection_id, None)
            .await?;

        // SST flush publication owns its follow-up training/compaction morsel.
        // Calling compact_collection here as well races that admitted morsel on
        // the same L0 source.  At 3.3M rows this produced two full reads, sorts,
        // encodes and L1 writes, followed by a 6.6M-row deduplicating L1→L2
        // merge.  Return after durable flush publication and let the SST worker
        // pool perform the single admitted maintenance action.
        if flush_owns_compaction_schedule(self.storage_engine.strategy()) {
            debug!(
                "SST flush owns asynchronous compaction scheduling for collection {}",
                collection_id
            );
            return Ok(());
        }

        // Trigger compaction for this collection
        let collection_object_id: crate::core::stable_id::CollectionObjectId =
            collection_id.parse().map_err(|error| {
                anyhow::anyhow!(
                    "force flush requires a numeric catalog object id, got {collection_id:?}: {error}"
                )
            })?;
        if let Some(collection) = self.collection_cache.get(&collection_object_id) {
            match self
                .storage_engine
                .compact_collection(collection_id, Some(&**collection))
                .await
            {
                Ok(result) => {
                    info!(
                        "✅ Compacted collection {}: {} files created, {} files processed",
                        collection_id,
                        result.output_files.unwrap_or(0),
                        result.input_files.unwrap_or(0)
                    );
                }
                Err(e) => {
                    debug!(
                        "⚠️ Compaction failed for collection {}: {}",
                        collection_id, e
                    );
                    // Don't fail the entire flush operation due to compaction issues
                }
            }
        } else {
            debug!(
                "⚠️ Collection {} not found in cache, skipping compaction",
                collection_id
            );
        }

        debug!("Force flush for collection {} completed", collection_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sst_flush_is_the_single_compaction_scheduler() {
        assert!(flush_owns_compaction_schedule(StorageFormatStrategy::Sst));
        assert!(!flush_owns_compaction_schedule(
            StorageFormatStrategy::Viper
        ));
    }
}
