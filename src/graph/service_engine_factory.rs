//! # Graph Engine Factory
//!
//! Centralizes graph engine creation, selection, and first-time initialization.
//!
//! ## Engine Selection
//!
//! | Engine | Status | Recommended |
//! |--------|--------|-------------|
//! | ORION | Production | Yes (default) |
//! | PULSAR | Experimental | No |
//! | QUASAR | Experimental | No |
//!
//! ## Default Engine
//!
//! **ORION is the default and recommended graph engine for all workloads.**
//!
//! When no engine type is specified in collection metadata, ORION is automatically
//! selected. ORION provides:
//! - Production-grade reliability with WAL persistence
//! - High performance (1M+ edges/sec traversal, <1us node lookup)
//! - Full feature support (graph algorithms, label indexes, concurrent access)
//!
//! ## Experimental Engines
//!
//! PULSAR and QUASAR are experimental and should only be used for research/development:
//!
//! - **PULSAR**: Distributed sharding with incomplete cross-shard query support
//! - **QUASAR**: Hot/cold tiering with minimal tiering logic and no WAL
//!
//! Requesting an experimental engine will log a warning.
//!
//! ## Responsibilities
//!
//! - Resolve engine type from collection metadata (ORION/PULSAR/QUASAR)
//! - Normalize storage root URL and pass through to persistence layer
//! - Initialize schema-derived unique/multi-unique indexes on first load
//! - Log warnings when experimental engines are requested

use super::Result;
use crate::core::error::ProximaDBError;
use crate::graph::engines::GraphEngine;
use std::sync::Arc;

impl super::GraphOperationsService {
    /// Get or create a graph engine for the specified graph ID
    pub(crate) async fn get_or_create_graph_engine(
        &self,
        graph_id: &str,
    ) -> Result<Arc<crate::graph::engines::GraphEngineImpl>> {
        if let Some(engine) = self.graphs.get(graph_id) {
            return Ok(Arc::clone(&engine));
        }

        // Verify graph collection exists first
        let collection = self
            .collection_service
            .ensure_graph_exists(graph_id)
            .await?;

        // Determine engine type and storage ROOT URL
        let engine_type_str = collection
            .engine_config
            .as_ref()
            .map_or_else(|| "ORION".to_string(), |cfg| cfg.engine_type.clone());

        let storage_root_url = collection
            .storage_config
            .as_ref()
            .map_or_else(|| self.base_storage_url.clone(), |cfg| cfg.base_url.clone());

        tracing::info!(
            "Creating new graph engine for '{}' type={} storage_root={}",
            graph_id,
            engine_type_str,
            storage_root_url
        );

        // Create engine based on type (default ORION - production-ready)
        // PULSAR and QUASAR are experimental and log warnings
        let engine_impl = match engine_type_str.to_ascii_uppercase().as_str() {
            "PULSAR" => {
                #[cfg(feature = "distributed-graph")]
                {
                    // WARNING: PULSAR is experimental
                    tracing::warn!(
                        "PULSAR engine requested for graph '{}' - PULSAR is EXPERIMENTAL. \
                         Cross-shard queries may be incomplete. For production, use ORION.",
                        graph_id
                    );
                    let cfg = crate::graph::engines::pulsar::PulsarConfig::default();
                    let pulsar = crate::graph::engines::pulsar::PulsarGraphEngine::new(cfg)?;
                    crate::graph::engines::GraphEngineImpl::Pulsar(pulsar)
                }
                #[cfg(not(feature = "distributed-graph"))]
                {
                    return Err(crate::core::error::ProximaDBError::NotImplemented(
                        "PULSAR engine requires 'distributed-graph' feature".to_string(),
                    ));
                }
            }
            "QUASAR" => {
                #[cfg(feature = "tiered-graph")]
                {
                    // WARNING: QUASAR is experimental
                    tracing::warn!(
                        "QUASAR engine requested for graph '{}' - QUASAR is EXPERIMENTAL. \
                         No WAL persistence, data loss possible. For production, use ORION.",
                        graph_id
                    );
                    // Derive a graph-scoped cold tier path under the configured storage root
                    let mut cfg = crate::graph::engines::quasar::QuasarConfig::default();
                    if storage_root_url.starts_with("file://") {
                        let base_path = storage_root_url.trim_start_matches("file://");
                        cfg.cold_tier_path = std::path::PathBuf::from(base_path)
                            .join("graphs")
                            .join(graph_id)
                            .join("quasar_cold");
                    }
                    let quasar = crate::graph::engines::quasar::QuasarGraphEngine::new(cfg).await?;
                    crate::graph::engines::GraphEngineImpl::Quasar(quasar)
                }
                #[cfg(not(feature = "tiered-graph"))]
                {
                    return Err(crate::core::error::ProximaDBError::NotImplemented(
                        "QUASAR engine requires 'tiered-graph' feature".to_string(),
                    ));
                }
            }
            _ => {
                let orion = crate::graph::OrionGraphEngine::with_persistence_for_graph(
                    graph_id.to_string(),
                    storage_root_url,
                    true,
                )
                .await?;
                crate::graph::engines::GraphEngineImpl::Orion(orion)
            }
        };

        let engine = Arc::new(engine_impl);
        self.graphs
            .insert(graph_id.to_string(), Arc::clone(&engine));

        // Initialize schema-derived constraints (unique, multi-unique) if present
        if let Some(coll) = self.collection_service.get_graph(graph_id).await?
            && let Some(schema) = &coll.schema
        {
            self.initialize_schema_constraints(graph_id, engine.as_ref(), schema)
                .await?;
        }

        Ok(engine)
    }

    /// Initialize constraint registries based on graph schema (unique constraints)
    pub(super) async fn initialize_schema_constraints(
        &self,
        graph_id: &str,
        engine: &crate::graph::engines::GraphEngineImpl,
        schema: &crate::proto::proximadb_v1::GraphSchema,
    ) -> Result<()> {
        use dashmap::DashMap;
        // Build composite (multi-property) unique constraint indexes
        for uc in &schema.unique_constraints {
            if uc.properties.is_empty() {
                continue;
            }
            let labels_key = Self::normalize_list(&uc.node_labels);
            let props_key = Self::normalize_list(&uc.properties);
            // Always use a normalized (sorted) property list to build composite keys
            let props_sorted: Vec<String> = props_key.split('|').map(|s| s.to_string()).collect();
            let map: DashMap<String, String> = DashMap::new();
            let existing_nodes = engine.get_all_nodes()?;
            for node in existing_nodes {
                if !Self::node_has_all_labels(&node, &uc.node_labels) {
                    continue;
                }
                if let Some(composite) = Self::composite_key_for_node(&node, &props_sorted) {
                    if let Some(existing) = map.get(&composite)
                        && existing.value() != &node.id
                    {
                        return Err(ProximaDBError::InvalidInput(format!(
                            "Existing duplicate composite value '{}' for unique {:?}",
                            composite, uc.properties
                        )));
                    }
                    map.insert(composite, node.id.clone());
                }
            }
            self.memory_pool
                .unique_constraints_multi
                .insert((graph_id.to_string(), labels_key, props_key), map);
        }
        Ok(())
    }
}
