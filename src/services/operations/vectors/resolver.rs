//! Collection-resolution collaborator extracted from `VectorOperationsService`
//! (Phase 2.1 god-object decomposition, slice 3).
//!
//! Resolves a user-facing collection identifier to its loaded `Collection`
//! (cached, registering it with the WAL on first load) and to its configured
//! storage engine (cached). `VectorOperationsService` keeps its public surface
//! (`resolve_collection_id`/`resolve_collection_name`/`get_engine_for_collection`/
//! `invalidate_collection_cache`) and its private `get_or_load_collection`
//! (~17 internal callers) and delegates here.

use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use tracing::{debug, info};

use crate::proto::proximadb_v1::Collection;
use crate::storage::persistence::write_ahead_log::WriteAheadLogManager;
use crate::storage::traits::UnifiedStorageFormat;

/// Owns the collection-metadata and per-collection storage-engine caches plus
/// the ports needed to populate them. Holds only `Arc` handles; the caches are
/// shared with `VectorOperationsService` (same underlying maps).
pub(crate) struct CollectionResolver {
    collection_cache: Arc<DashMap<String, Arc<Collection>>>,
    engine_cache: Arc<DashMap<String, Arc<dyn UnifiedStorageFormat>>>,
    collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
    wal_manager: Arc<WriteAheadLogManager>,
}

impl CollectionResolver {
    pub(crate) fn new(
        collection_cache: Arc<DashMap<String, Arc<Collection>>>,
        engine_cache: Arc<DashMap<String, Arc<dyn UnifiedStorageFormat>>>,
        collection_port: Arc<dyn proximadb_runtime::CollectionPort>,
        wal_manager: Arc<WriteAheadLogManager>,
    ) -> Self {
        Self {
            collection_cache,
            engine_cache,
            collection_port,
            wal_manager,
        }
    }

    /// Load a collection by id (cache hit, else fetch via the collection port
    /// and register it with the WAL manager), caching the result.
    pub(crate) async fn get_or_load_collection(
        &self,
        collection_id: &str,
    ) -> Result<Arc<Collection>> {
        let collection_id_string = collection_id.to_string();
        if let Some(cached) = self.collection_cache.get(&collection_id_string) {
            Ok(cached.clone())
        } else {
            // Load from collection service
            let collection = self
                .collection_port
                .get_collection(collection_id, None)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Collection {} not found", collection_id))?;

            // Register collection with WAL manager for persistence
            if let Some(ref storage_assignment) = collection.storage_assignment
                && let Some(ref config) = collection.config
            {
                // Build compression_config from storage_config if available
                let compression_config = config.storage_config.as_ref().and_then(|sc| {
                    sc.compression.map(|alg| {
                        crate::proto::proximadb_v1::CompressionConfig {
                            algorithm: alg,
                            level: Some(3), // default level
                            adaptive: false,
                            min_ratio: None,
                            enable_quantization: false,
                            quantization_type: None,
                            normalization_method: None,
                            block_size_kb: 64,
                            dynamic_block_sizing: false,
                        }
                    })
                });

                // Convert distance_metric from Option<i32> to DistanceMetric
                let distance_metric = config
                    .distance_metric
                    .and_then(|m| crate::proto::proximadb_v1::DistanceMetric::try_from(m).ok())
                    .unwrap_or(crate::proto::proximadb_v1::DistanceMetric::Cosine);

                let assignment =
                    crate::storage::persistence::write_ahead_log::CollectionAssignment {
                        base_location: storage_assignment.base_location.clone(),
                        storage_engine: crate::proto::proximadb_v1::StorageEngine::try_from(
                            storage_assignment.engine,
                        )
                        .unwrap_or(crate::proto::proximadb_v1::StorageEngine::Sst),
                        dimension: config.dimension as i32,
                        compression_config,
                        distance_metric,
                    };
                self.wal_manager
                    .assign_collection(collection_id_string.clone(), assignment)
                    .await;
                tracing::debug!(
                    "✅ Registered collection {} with WAL manager",
                    collection_id
                );
            }

            let arc_collection = Arc::new(collection);
            self.collection_cache
                .insert(collection_id_string, arc_collection.clone());
            Ok(arc_collection)
        }
    }

    /// Get or create the correct storage engine for a collection.
    ///
    /// This is CRITICAL for multi-engine support:
    /// - Looks up the collection's configured engine type from its storage_assignment
    /// - Creates the engine if not already cached
    /// - Returns the cached engine for subsequent calls
    ///
    /// Without this, all searches would use SST regardless of collection configuration.
    pub(crate) async fn get_engine_for_collection(
        &self,
        collection_id: &str,
    ) -> Result<Arc<dyn UnifiedStorageFormat>> {
        // Check cache first
        if let Some(engine) = self.engine_cache.get(collection_id) {
            return Ok(engine.clone());
        }

        // Get collection to determine engine type
        let collection = self.get_or_load_collection(collection_id).await?;

        // Determine engine type from storage_assignment
        let engine_type = collection.storage_assignment.as_ref().map_or(
            crate::proto::proximadb_v1::StorageEngine::Sst,
            |sa| {
                crate::proto::proximadb_v1::StorageEngine::try_from(sa.engine)
                    .unwrap_or(crate::proto::proximadb_v1::StorageEngine::Sst)
            },
        );

        debug!(
            "🔧 Creating storage engine {:?} for collection {}",
            engine_type, collection_id
        );

        // Create the appropriate engine
        let engine =
            crate::storage::engines::factory::StorageFormatFactory::create_from_proto_async(
                engine_type,
            )
            .await?;

        // Cache it for future use
        self.engine_cache
            .insert(collection_id.to_string(), engine.clone());

        info!(
            "✅ Cached storage engine {:?} for collection {}",
            engine_type, collection_id
        );

        Ok(engine)
    }

    /// Resolve a user-facing collection identifier (name) to the canonical
    /// internal id; falls back to the input if resolution fails.
    pub(crate) async fn resolve_collection_id(&self, identifier: &str) -> String {
        match self.get_or_load_collection(identifier).await {
            Ok(collection) => collection.id.clone(),
            Err(_) => identifier.to_string(),
        }
    }

    /// Reverse of [`resolve_collection_id`]: resolve an internal id (or name) to
    /// the user-facing collection **name**. Returns `None` if the collection
    /// can't be loaded or carries no config.
    pub(crate) async fn resolve_collection_name(&self, identifier: &str) -> Option<String> {
        let collection = self.get_or_load_collection(identifier).await.ok()?;
        collection.config.as_ref().map(|cfg| cfg.name.clone())
    }

    /// Drop a collection's cached metadata so the next load re-fetches it.
    pub(crate) fn invalidate_collection_cache(&self, collection_id: &str) {
        self.collection_cache.remove(collection_id);
        tracing::debug!("🗑️ Invalidated collection cache for '{}'", collection_id);
    }
}
