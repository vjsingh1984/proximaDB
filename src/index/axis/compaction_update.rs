// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Post-compaction index reconciliation (AXIS side).
//!
//! When storage finishes a compaction it announces the change via the
//! `IndexMaintenance::update_indexes_after_compaction` port; the *mechanism*
//! lives here, inside AXIS, so the concrete `Arc<dyn AxisVectorIndex>` reader
//! handles never cross into `src/storage`. Moved verbatim from the former
//! `storage::…::CompactionAxisUpdater` (behaviour-neutral) as part of the
//! storage→index dependency inversion (Slice D / IndexPort).

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::index::axis::{AxisManager, AxisVectorIndex};
use proximadb_records::ProximaRecord;

impl AxisManager {
    /// Reconcile this collection's indexes after a storage compaction: apply
    /// `deleted_vector_ids` removals, re-index `merged_vectors`, and rebuild any
    /// static index that can't mutate in place. AXIS owns its readers throughout.
    pub async fn update_indexes_after_compaction(
        &self,
        collection_id: &str,
        deleted_vector_ids: &[String],
        merged_vectors: &[ProximaRecord],
    ) -> Result<()> {
        let indexes = self.get_collection_indexes(collection_id).await?;
        if indexes.is_empty() {
            debug!(
                "No indexes found for collection {}, skipping",
                collection_id
            );
            return Ok(());
        }

        info!(
            "🔄 AXIS Compaction: Updating {} indexes for collection {}",
            indexes.len(),
            collection_id
        );

        if !deleted_vector_ids.is_empty() {
            remove_deleted_vectors_from_indexes(&indexes, deleted_vector_ids, collection_id)
                .await?;
        }

        if !merged_vectors.is_empty() {
            update_merged_vectors_in_indexes(&indexes, merged_vectors, collection_id).await?;
        }

        rebuild_static_indexes_if_needed(self, collection_id, &indexes).await?;

        info!(
            "✅ AXIS Compaction: Successfully updated {} indexes for collection {}",
            indexes.len(),
            collection_id
        );

        Ok(())
    }
}

/// Remove deleted vectors from all indexes
async fn remove_deleted_vectors_from_indexes(
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
    axis: &AxisManager,
    collection_id: &str,
    indexes: &[(String, Arc<dyn AxisVectorIndex>)],
) -> Result<()> {
    for (index_name, index) in indexes {
        // Check if this is a static index (like Annoy)
        let algorithm = index.algorithm();
        if matches!(
            algorithm,
            proximadb_index_types::IndexAlgorithm::Annoy { .. }
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
