/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # TITAN Graph Storage Engine
//!
//! **STATUS**: Factory-registration stub (May 2026 — reclassified)
//!
//! Traversal-Indexed Topology and Adjacency Network
//!
//! ## Classification (Convergence Mandate)
//!
//! TITAN is a **factory registration stub** only.  The in-memory adjacency
//! ownership that was previously carried by the deleted `TitanGraphEngine`
//! struct has been retired in favour of
//! `crate::graph::adjacency_projection::InMemoryGraphAdjacencyProjection`,
//! which is owned by `GraphOperationsService` and is explicitly rebuildable
//! from canonical `ProximaRecord` edge records.
//!
//! TITAN must NOT become an independent durable graph store with its own WAL,
//! transaction semantics, or adjacency authority.  If an LSM-backed durable
//! graph projection is needed in the future, it should be cataloged as a
//! physical projection over canonical records and proved necessary via ADR.
//!
//! ## Current Role
//!
//! `TitanEngine` implements `UnifiedStorageFormat` with empty stubs so the
//! engine identifier `"titan"` can be registered through the standard factory
//! without panicking.  All meaningful graph traversal work is delegated to the
//! graph service layer via `InMemoryGraphAdjacencyProjection`.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

use crate::core::search::results::OptimizedSearchRecord;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageFormatStrategy,
    StorageQueryContext, UnifiedStorageFormat,
};

// ---------------------------------------------------------------------------
// TitanEngine -- thin UnifiedStorageFormat wrapper for factory registration
// ---------------------------------------------------------------------------

/// Factory-registration stub for the TITAN engine identifier.
///
/// All vector operations return empty results.  Graph adjacency projection is
/// provided by `InMemoryGraphAdjacencyProjection` in the graph service layer,
/// not by this struct.
pub struct TitanEngine;

impl TitanEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TitanEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// UnifiedStorageFormat implementation (stubs)
// ---------------------------------------------------------------------------

#[async_trait]
impl UnifiedStorageFormat for TitanEngine {
    fn engine_name(&self) -> &'static str {
        "titan"
    }

    fn engine_version(&self) -> &'static str {
        "0.1.0"
    }

    fn strategy(&self) -> StorageFormatStrategy {
        StorageFormatStrategy::Sst
    }

    fn get_filesystem_factory(&self) -> &FilesystemFactory {
        use std::sync::OnceLock;
        static FACTORY: OnceLock<FilesystemFactory> = OnceLock::new();
        use futures::executor::block_on;

        FACTORY.get_or_init(|| {
            block_on(async {
                FilesystemFactory::create(FilesystemConfig::default())
                    .await
                    .unwrap_or_else(|_| {
                        #[allow(clippy::panic)]
                        {
                            panic!("Failed to create filesystem factory for TITAN engine")
                        }
                    })
            })
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        metrics.insert(
            "engine".to_string(),
            serde_json::Value::String("titan".to_string()),
        );
        metrics.insert(
            "status".to_string(),
            serde_json::Value::String("stub".to_string()),
        );
        Ok(metrics)
    }

    async fn vector_by_id(
        &self,
        collection_id: &str,
        base_path: &str,
        vector_id: &str,
    ) -> Result<Option<proximadb_records::ProximaRecord>> {
        let _ = (collection_id, base_path, vector_id);
        Ok(None)
    }

    async fn search_vectors_unified(
        &self,
        ctx: &StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        let _ = ctx;
        Ok(vec![])
    }

    async fn do_flush(&self, _params: &FlushParameters) -> Result<FlushResult> {
        Ok(FlushResult::default())
    }

    async fn do_compact(&self, _params: &CompactionParameters) -> Result<CompactionResult> {
        Ok(CompactionResult::default())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_titan_engine_name() {
        let engine = TitanEngine::new();
        assert_eq!(engine.engine_name(), "titan");
    }

    #[test]
    fn test_titan_engine_strategy() {
        let engine = TitanEngine::new();
        assert_eq!(engine.strategy(), StorageFormatStrategy::Sst);
    }

    #[tokio::test]
    async fn test_titan_metrics_has_stub_status() {
        let engine = TitanEngine::new();
        let metrics = engine.collect_engine_metrics().await.expect("metrics ok");
        assert_eq!(
            metrics.get("status"),
            Some(&serde_json::Value::String("stub".to_string()))
        );
    }

    #[tokio::test]
    async fn test_titan_vector_by_id_returns_none() {
        let engine = TitanEngine::new();
        let result = engine
            .vector_by_id("col", "/tmp", "v1")
            .await
            .expect("no error");
        assert!(result.is_none());
    }
}
