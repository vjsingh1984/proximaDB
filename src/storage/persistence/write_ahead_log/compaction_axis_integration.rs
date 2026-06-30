/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! AXIS Index Integration for Compaction Process
//!
//! Thin storage-side façade over the `IndexMaintenance` port: storage announces
//! post-compaction changes and reads index stats; the index side (AXIS) owns the
//! reader mechanics. The former in-storage reader manipulation (remove/add/rebuild
//! on live `Arc<dyn AxisVectorIndex>` handles) moved to
//! `crate::index::axis::compaction_update` as part of the storage→index
//! dependency inversion — storage now depends only on the foundation port + DTO.

use std::sync::Arc;

use anyhow::Result;

use proximadb_index_traits::{IndexMaintenance, IndexReaderSnapshot};
use proximadb_index_types::IndexAlgorithm;
use proximadb_records::ProximaRecord;

use crate::storage::traits::CompactionResult;

/// AXIS index updater for compaction operations.
#[derive(Clone)]
pub struct CompactionAxisUpdater {
    /// Index-maintenance port; the concrete AXIS impl is injected by the
    /// composition root. `None` disables index updates (no index configured).
    axis_manager: Option<Arc<dyn IndexMaintenance>>,
}

impl CompactionAxisUpdater {
    /// Create a new AXIS updater for compaction.
    pub fn new(axis_manager: Option<Arc<dyn IndexMaintenance>>) -> Self {
        Self { axis_manager }
    }

    /// Update AXIS indexes after compaction.
    ///
    /// Delegates the reconciliation (deletions, merged re-indexing, static-index
    /// rebuilds) to the index side via the port; the live index readers never
    /// cross into storage.
    pub async fn update_indexes_after_compaction(
        &self,
        collection_id: &str,
        _compaction_result: &CompactionResult,
        deleted_vector_ids: &[String],
        merged_vectors: &[ProximaRecord],
    ) -> Result<()> {
        match &self.axis_manager {
            Some(axis) => {
                axis.update_indexes_after_compaction(
                    collection_id,
                    deleted_vector_ids,
                    merged_vectors,
                )
                .await
            }
            None => {
                tracing::debug!("No AXIS manager configured, skipping index updates");
                Ok(())
            }
        }
    }

    /// Get compaction statistics for AXIS indexes.
    pub async fn get_compaction_stats(&self, collection_id: &str) -> Result<CompactionIndexStats> {
        let Some(axis) = &self.axis_manager else {
            return Ok(CompactionIndexStats::default());
        };
        let snapshots = axis.collection_index_stats(collection_id).await?;
        Ok(aggregate_index_stats(&snapshots))
    }
}

/// Aggregate per-index snapshots into compaction stats.
///
/// Pure (no I/O), so it is unit-testable in isolation: classifies HNSW/IVF/LSH as
/// dynamic and Annoy as static, summing vector counts and memory across indexes.
pub(crate) fn aggregate_index_stats(snapshots: &[IndexReaderSnapshot]) -> CompactionIndexStats {
    let mut stats = CompactionIndexStats {
        total_indexes: snapshots.len(),
        ..Default::default()
    };

    for snap in snapshots {
        stats.total_vectors_indexed += snap.vector_count;
        stats.total_memory_usage_bytes += snap.memory_usage_bytes;

        match &snap.algorithm {
            IndexAlgorithm::HNSW { .. }
            | IndexAlgorithm::IVF { .. }
            | IndexAlgorithm::LSH { .. } => stats.dynamic_indexes += 1,
            IndexAlgorithm::Annoy { .. } => stats.static_indexes += 1,
            _ => {}
        }
    }

    stats
}

/// Statistics for AXIS indexes during compaction.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CompactionIndexStats {
    pub total_indexes: usize,
    pub dynamic_indexes: usize,
    pub static_indexes: usize,
    pub total_vectors_indexed: usize,
    pub total_memory_usage_bytes: usize,
}

#[cfg(test)]
#[path = "compaction_axis_integration_tests.rs"]
mod tests;
