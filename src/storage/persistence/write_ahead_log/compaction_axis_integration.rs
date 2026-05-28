/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! AXIS Index Integration for Compaction Process
//!
//! This module handles index updates during compaction operations,
//! ensuring that AXIS indexes remain consistent with the compacted data.

use anyhow::Result;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::index::axis::{AxisManager, AxisVectorIndex};
use crate::storage::traits::CompactionResult;
use proximadb_records::ProximaRecord;

/// AXIS index updater for compaction operations
#[derive(Clone)]
pub struct CompactionAxisUpdater {
    /// AXIS manager for index operations
    axis_manager: Option<Arc<AxisManager>>,
}

impl CompactionAxisUpdater {
    /// Create a new AXIS updater for compaction
    pub fn new(axis_manager: Option<Arc<AxisManager>>) -> Self {
        Self { axis_manager }
    }

    /// Update AXIS indexes after compaction
    ///
    /// This method handles:
    /// 1. Removing deleted vectors from indexes
    /// 2. Re-indexing merged/updated vectors
    /// 3. Rebuilding static indexes (like Annoy) if needed
    pub async fn update_indexes_after_compaction(
        &self,
        collection_id: &str,
        _compaction_result: &CompactionResult,
        deleted_vector_ids: &[String],
        merged_vectors: &[ProximaRecord],
    ) -> Result<()> {
        let axis = match &self.axis_manager {
            Some(manager) => manager,
            None => {
                debug!("No AXIS manager configured, skipping index updates");
                return Ok(());
            }
        };

        info!(
            "🔄 AXIS Compaction: Updating indexes for collection {} after compaction_info",
            collection_id
        );

        // Get all indexes for the collection
        let indexes = axis.get_collection_indexes(collection_id).await?;
        if indexes.is_empty() {
            debug!(
                "No indexes found for collection {}, skipping",
                collection_id
            );
            return Ok(());
        }

        info!(
            "🔄 AXIS Compaction: Found {} indexes to update for collection {}",
            indexes.len(),
            collection_id
        );

        // Process deletions first
        if !deleted_vector_ids.is_empty() {
            self.remove_deleted_vectors_from_indexes(&indexes, deleted_vector_ids, collection_id)
                .await?;
        }

        // Process merged/updated vectors
        if !merged_vectors.is_empty() {
            self.update_merged_vectors_in_indexes(&indexes, merged_vectors, collection_id)
                .await?;
        }

        // Handle static indexes that need rebuilding
        self.rebuild_static_indexes_if_needed(axis, collection_id, &indexes)
            .await?;

        info!(
            "✅ AXIS Compaction: Successfully updated {} indexes for collection {}",
            indexes.len(),
            collection_id
        );

        Ok(())
    }

    /// Remove deleted vectors from all indexes
    async fn remove_deleted_vectors_from_indexes(
        &self,
        indexes: &[(String, Arc<dyn AxisVectorIndex>)],
        deleted_vector_ids: &[String],
        collection_id: &str,
    ) -> Result<()> {
        info!(
            "🗑️ AXIS Compaction: Removing {} deleted vectors from indexes for {}",
            deleted_vector_ids.len(),
            collection_id
        );

        let _deleted_set: HashSet<&str> = deleted_vector_ids.iter().map(|s| s.as_str()).collect();
        let mut removal_errors = Vec::new();

        for (index_name, index) in indexes {
            debug!(
                "Removing deleted vectors from index {} for collection {}",
                index_name, collection_id
            );

            for vector_id in deleted_vector_ids {
                match index.remove(vector_id).await {
                    Ok(_) => {
                        debug!("Removed vector {} from index {}", vector_id, index_name);
                    }
                    Err(e) => {
                        // Some indexes (like Annoy) don't support removal
                        if e.to_string().contains("does not support removal") {
                            debug!(
                                "Index {} is static and doesn't support removal, will need rebuild",
                                index_name
                            );
                            break; // No point trying more removals on this index
                        } else {
                            warn!(
                                "Failed to remove vector {} from index {}: {}",
                                vector_id, index_name, e
                            );
                            removal_errors.push((vector_id.clone(), index_name.clone(), e));
                        }
                    }
                }
            }
        }

        // Log removal statistics
        if !removal_errors.is_empty() {
            warn!(
                "⚠️ AXIS Compaction: {} removal errors occurred during compaction_info",
                removal_errors.len()
            );
        }

        Ok(())
    }

    /// Update merged vectors in all indexes
    async fn update_merged_vectors_in_indexes(
        &self,
        indexes: &[(String, Arc<dyn AxisVectorIndex>)],
        merged_vectors: &[ProximaRecord],
        collection_id: &str,
    ) -> Result<()> {
        info!(
            "🔄 AXIS Compaction: Updating {} merged vectors in indexes for {}",
            merged_vectors.len(),
            collection_id
        );

        let mut update_errors = Vec::new();

        for (index_name, index) in indexes {
            debug!(
                "Updating merged vectors in index {} for collection {}",
                index_name, collection_id
            );

            for vector in merged_vectors {
                let vector_id = &vector.oid;
                let Some(embedding) = vector.embeddings.first() else {
                    debug!(
                        "Skipping merged record {} with no embedding for index {}",
                        vector_id, index_name
                    );
                    continue;
                };

                // Remove old version first (if it exists)
                let _ = index.remove(vector_id).await; // Ignore errors as it might not exist

                // Add updated version
                match index
                    .add(vector_id.clone(), embedding.values.to_fp32_owned())
                    .await
                {
                    Ok(_) => {
                        debug!("Updated vector {} in index {}", vector_id, index_name);
                    }
                    Err(e) => {
                        // Some indexes (like Annoy) don't support dynamic updates
                        if e.to_string().contains("cannot be modified") {
                            debug!(
                                "Index {} is static and doesn't support updates, will need rebuild",
                                index_name
                            );
                            break; // No point trying more updates on this index
                        } else {
                            warn!(
                                "Failed to update vector {} in index {}: {}",
                                vector_id, index_name, e
                            );
                            update_errors.push((vector_id.clone(), index_name.clone(), e));
                        }
                    }
                }
            }
        }

        // Log update statistics
        if !update_errors.is_empty() {
            warn!(
                "⚠️ AXIS Compaction: {} update errors occurred during compaction_info",
                update_errors.len()
            );
        }

        Ok(())
    }

    /// Rebuild static indexes that don't support dynamic updates
    async fn rebuild_static_indexes_if_needed(
        &self,
        axis: &AxisManager,
        collection_id: &str,
        indexes: &[(String, Arc<dyn AxisVectorIndex>)],
    ) -> Result<()> {
        for (index_name, index) in indexes {
            // Check if this is a static index (like Annoy)
            let algorithm = index.algorithm();
            if matches!(
                algorithm,
                crate::index::axis::types::IndexAlgorithm::Annoy { .. }
            ) {
                info!(
                    "🔨 AXIS Compaction: Rebuilding static index {} for collection {}",
                    index_name, collection_id
                );

                // Trigger a full rebuild through AXIS manager
                match axis.rebuild_index(collection_id, index_name).await {
                    Ok(_) => {
                        info!(
                            "✅ Successfully rebuilt static index {} for collection {}",
                            index_name, collection_id
                        );
                    }
                    Err(e) => {
                        warn!(
                            "❌ Failed to rebuild static index {} for collection {}: {}",
                            index_name, collection_id, e
                        );
                        // Continue with other indexes even if one fails
                    }
                }
            }
        }

        Ok(())
    }

    /// Get compaction statistics for AXIS indexes
    pub async fn get_compaction_stats(&self, collection_id: &str) -> Result<CompactionIndexStats> {
        let axis = match &self.axis_manager {
            Some(manager) => manager,
            None => {
                return Ok(CompactionIndexStats::default());
            }
        };

        let indexes = axis.get_collection_indexes(collection_id).await?;

        let mut stats = CompactionIndexStats {
            total_indexes: indexes.len(),
            ..Default::default()
        };

        for (_name, index) in indexes {
            let index_stats = index.stats();
            stats.total_vectors_indexed += index_stats.vector_count;
            stats.total_memory_usage_bytes += index_stats.memory_usage_bytes;

            // Count index types
            match index.algorithm() {
                crate::index::axis::types::IndexAlgorithm::HNSW { .. } => {
                    stats.dynamic_indexes += 1
                }
                crate::index::axis::types::IndexAlgorithm::IVF { .. } => stats.dynamic_indexes += 1,
                crate::index::axis::types::IndexAlgorithm::LSH { .. } => stats.dynamic_indexes += 1,
                crate::index::axis::types::IndexAlgorithm::Annoy { .. } => {
                    stats.static_indexes += 1
                }
                _ => {}
            }
        }

        Ok(stats)
    }
}

/// Statistics for AXIS indexes during compaction
#[derive(Debug, Default)]
pub struct CompactionIndexStats {
    pub total_indexes: usize,
    pub dynamic_indexes: usize,
    pub static_indexes: usize,
    pub total_vectors_indexed: usize,
    pub total_memory_usage_bytes: usize,
}

// Test module moved to separate file for better organization
#[cfg(test)]
#[path = "compaction_axis_integration_tests.rs"]
mod tests;
