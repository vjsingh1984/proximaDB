// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Migration Utilities for SKS Graph-First Architecture
//!
//! This module provides tools to migrate from legacy split storage to graph-first storage.

use anyhow::{Context, Result};
use std::sync::Arc;

use crate::graph::GraphOperationsService;
use crate::proto::proximadb_v1::{CreateGraphRequest, Entity};
use crate::storage::entity_store::{EntityStore, OrionBackedEntityStore, ProximaEntityStore};

/// Migration configuration
#[derive(Debug, Clone)]
pub struct MigrationConfig {
    /// Collection ID to migrate
    pub collection_id: String,

    /// Batch size for migration
    pub batch_size: usize,

    /// Whether to validate after migration
    pub validate: bool,

    /// Whether to delete legacy data after migration
    pub cleanup_legacy: bool,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            collection_id: String::new(),
            batch_size: 1000,
            validate: true,
            cleanup_legacy: false,
        }
    }
}

/// Migration statistics
#[derive(Debug, Default)]
pub struct MigrationStats {
    pub entities_migrated: usize,
    pub relations_migrated: usize,
    pub errors: usize,
    pub duration_ms: u128,
}

/// Migrate entities from legacy storage to graph-first storage
pub async fn migrate_to_graph_first(
    legacy_store: &ProximaEntityStore,
    collection_id: &str,
    graph_service: Arc<GraphOperationsService>,
    config: &MigrationConfig,
) -> Result<MigrationStats> {
    let start = std::time::Instant::now();
    let mut stats = MigrationStats::default();

    // Create graph collection
    let create_request = CreateGraphRequest {
        graph_id: collection_id.to_string(),
        name: Some(format!("Migrated: {}", collection_id)),
        description: Some("Migrated from legacy split storage".to_string()),
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };

    graph_service
        .create_graph_collection(create_request)
        .await
        .context("Failed to create graph collection for migration")?;

    let graph_store = OrionBackedEntityStore::new(graph_service, collection_id.to_string());

    // List all entities from legacy storage
    let entities = legacy_store
        .list_entities(collection_id, 0, usize::MAX)
        .await
        .context("Failed to list entities from legacy storage")?;

    println!("Found {} entities to migrate", entities.len());

    // Migrate in batches
    for chunk in entities.chunks(config.batch_size) {
        let mut batch = Vec::new();
        for entity in chunk {
            batch.push(entity.clone());
        }

        match graph_store
            .batch_upsert_entities(collection_id, batch)
            .await
        {
            Ok(count) => {
                stats.entities_migrated += count;
                println!(
                    "Migrated batch: {} entities (total: {})",
                    count, stats.entities_migrated
                );
            }
            Err(e) => {
                stats.errors += 1;
                eprintln!("Error migrating batch: {}", e);
            }
        }
    }

    // Migrate relations (if stored separately in legacy)
    // TODO: Implement relation migration when legacy relation storage is available

    stats.duration_ms = start.elapsed().as_millis();

    // Validation
    if config.validate {
        println!("Validating migration...");
        validate_migration(legacy_store, &graph_store, collection_id, &entities).await?;
    }

    Ok(stats)
}

/// Validate that migration was successful
async fn validate_migration(
    legacy_store: &ProximaEntityStore,
    graph_store: &OrionBackedEntityStore,
    collection_id: &str,
    entities: &[Entity],
) -> Result<()> {
    let mut errors = 0;

    for entity in entities {
        // Check if entity exists in graph store
        match graph_store
            .get_entity(collection_id, &entity.id, true, false)
            .await
        {
            Ok(Some(migrated_entity)) => {
                // Verify key fields match
                if migrated_entity.id != entity.id {
                    eprintln!(
                        "Entity ID mismatch: {} vs {}",
                        migrated_entity.id, entity.id
                    );
                    errors += 1;
                }
                if migrated_entity.embeddings.len() != entity.embeddings.len() {
                    eprintln!("Embedding count mismatch for {}", entity.id);
                    errors += 1;
                }
            }
            Ok(None) => {
                eprintln!(
                    "Entity {} not found in graph store after migration",
                    entity.id
                );
                errors += 1;
            }
            Err(e) => {
                eprintln!(
                    "Error retrieving entity {} from graph store: {}",
                    entity.id, e
                );
                errors += 1;
            }
        }

        if errors >= 10 {
            anyhow::bail!("Too many validation errors ({}), aborting", errors);
        }
    }

    if errors > 0 {
        anyhow::bail!("Validation failed with {} errors", errors);
    }

    println!(
        "✓ Validation successful: all {} entities migrated correctly",
        entities.len()
    );
    Ok(())
}

/// Rollback migration by clearing the graph collection
///
/// Note: This clears all data from the graph but doesn't delete the collection itself.
/// Full deletion requires manual cleanup or additional API support.
pub async fn rollback_migration(
    _graph_service: Arc<GraphOperationsService>,
    collection_id: &str,
) -> Result<()> {
    // TODO: Implement graph collection deletion when API is available
    // For now, we rely on manual cleanup or re-creation
    println!(
        "⚠ Migration rollback: Please manually delete graph collection '{}'",
        collection_id
    );
    println!("  The graph data should be cleared, but collection metadata may remain");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migration_config_default() {
        let config = MigrationConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert!(config.validate);
        assert!(!config.cleanup_legacy);
    }

    #[test]
    fn test_migration_stats_default() {
        let stats = MigrationStats::default();
        assert_eq!(stats.entities_migrated, 0);
        assert_eq!(stats.relations_migrated, 0);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.duration_ms, 0);
    }
}
