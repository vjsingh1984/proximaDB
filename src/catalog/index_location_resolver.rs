// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Catalog adapter for AXIS's index-location read-port (CATALOG_OBJECT_MODEL #3).
//!
//! AXIS depends only on the catalog-free [`IndexLocationResolver`] trait; this
//! adapter — in the control layer — implements it over the catalog, inverting the
//! dependency so the index/storage plane never imports `proximadb-catalog`.
//!
//! Resolution is on demand: given a `collection_id`, it resolves the owning table
//! and namespace from the catalog and runs the same
//! [`ann_index_locations`](crate::storage::trait_components::path_resolver::ann_index_locations)
//! rule the boot adapter uses (explicit `projection.location` wins; the
//! `PROXIMADB_INDEX_CATALOG_PATHS` migration derives the `DrPathBuilder` default).
//! The caller ([`AxisManager`](crate::index::AxisManager)) memoizes the result, so
//! this is consulted at most once per collection.

use std::sync::Arc;

use async_trait::async_trait;

use crate::catalog::CatalogManager;
use crate::index::IndexLocationResolver;
use crate::storage::trait_components::path_resolver::ann_index_locations;

/// Resolves a collection's ANN index location from the catalog on demand — the
/// control-layer implementation of AXIS's [`IndexLocationResolver`] read-port.
pub struct CatalogIndexLocationResolver {
    catalog_manager: Arc<CatalogManager>,
    /// When true, ANN projections without an explicit `location` are migrated to
    /// the `DrPathBuilder` `indexes/<projection>/` layout; when false they keep the
    /// `index_persist_url`/`dir` convention (resolver returns `None`).
    migrate: bool,
}

impl CatalogIndexLocationResolver {
    pub fn new(catalog_manager: Arc<CatalogManager>, migrate: bool) -> Self {
        Self {
            catalog_manager,
            migrate,
        }
    }
}

#[async_trait]
impl IndexLocationResolver for CatalogIndexLocationResolver {
    async fn resolve_index_location(&self, collection_id: &str) -> Option<String> {
        let (catalog, table_id) = self
            .catalog_manager
            .resolve_table(collection_id)
            .await
            .ok()?;
        let namespace = catalog.get_namespace(&table_id.namespace).await.ok()?;
        let schema = catalog.get_table(&table_id).await.ok()?;
        // A collection has at most one ANN projection in AXIS's per-collection
        // index model; take the first resolved location (or None to fall back).
        ann_index_locations(&namespace, &schema, self.migrate)
            .into_iter()
            .next()
            .map(|(_collection, location)| location)
    }
}
