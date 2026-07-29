//! Canonical point-read collaborator for `VectorOperationsService`.
//!
//! Owns the WAL-to-persisted-engine boundary for get-by-id operations. A read
//! checks the write buffer first, then resolves the collection's persisted
//! storage assignment and delegates to that engine's neutral `point_lookup`
//! contract. Keeping this boundary in one place prevents protocol surfaces from
//! inventing SST-specific fallbacks that bypass HELIX/VIPER/NOVA or object-store
//! routing.

use std::sync::Arc;

use anyhow::Result;
use proximadb_records::ProximaRecord;

use crate::storage::persistence::write_ahead_log::WriteAheadLogManager;

use super::resolver::CollectionResolver;

/// Owns the handles required for a canonical point read. Constructed on demand
/// from cheap `Arc` clones by `VectorOperationsService::read_coordinator`.
pub(crate) struct VectorReadCoordinator {
    wal_manager: Arc<WriteAheadLogManager>,
    resolver: CollectionResolver,
}

impl VectorReadCoordinator {
    pub(crate) fn new(
        wal_manager: Arc<WriteAheadLogManager>,
        resolver: CollectionResolver,
    ) -> Self {
        Self {
            wal_manager,
            resolver,
        }
    }

    /// Fetch one canonical record by id, preferring the write buffer and then
    /// routing the persisted lookup through the collection's configured engine.
    pub(crate) async fn vector(
        &self,
        collection_id: &str,
        vector_id: &str,
        include_vector: bool,
        include_metadata: bool,
    ) -> Result<Option<ProximaRecord>> {
        let record = if let Some(record) = self
            .wal_manager
            .search_vector_by_id(collection_id, &vector_id.to_string())
            .await?
        {
            Some(record)
        } else {
            let collection = self.resolver.get_or_load_collection(collection_id).await?;
            let storage_assignment = collection.storage_assignment.as_ref();
            let base_path = storage_assignment
                .map(|assignment| assignment.base_location.as_str())
                .unwrap_or("");
            let identity = crate::storage::trait_components::path_resolver::typed_identity_from_storage_assignment(
                storage_assignment,
            );
            let engine = self
                .resolver
                .get_engine_for_collection(collection_id)
                .await?;

            engine
                .point_lookup(collection_id, base_path, &[vector_id.to_string()], identity)
                .await?
                .into_iter()
                .next()
        };

        Ok(record.map(|mut record| {
            if !include_vector {
                record.embeddings.clear();
            }
            if !include_metadata {
                record.props.clear();
            }
            record
        }))
    }

    /// Test whether an id exists through the exact same WAL/engine authority as
    /// get-by-id. Insert-only conflict detection must not grow a parallel read path.
    pub(crate) async fn contains(&self, collection_id: &str, vector_id: &str) -> Result<bool> {
        Ok(self
            .vector(collection_id, vector_id, false, false)
            .await?
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use dashmap::DashMap;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    use crate::storage::traits::{
        CompactionParameters, CompactionResult, FlushParameters, FlushResult,
        StorageFormatStrategy, StorageQueryContext, UnifiedStorageFormat,
    };

    use super::*;

    type PointLookupCall = (
        String,
        String,
        Vec<String>,
        Option<crate::core::stable_id::CollectionIdentity>,
    );

    struct PointLookupProbeEngine {
        record: ProximaRecord,
        calls: Arc<Mutex<Vec<PointLookupCall>>>,
    }

    #[async_trait]
    impl UnifiedStorageFormat for PointLookupProbeEngine {
        fn engine_name(&self) -> &'static str {
            "point-lookup-probe"
        }

        fn engine_version(&self) -> &'static str {
            "1"
        }

        fn strategy(&self) -> StorageFormatStrategy {
            StorageFormatStrategy::Helix
        }

        async fn do_flush(&self, _params: &FlushParameters) -> Result<FlushResult> {
            Ok(FlushResult::default())
        }

        async fn do_compact(&self, _params: &CompactionParameters) -> Result<CompactionResult> {
            Ok(CompactionResult::default())
        }

        async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
            Ok(HashMap::new())
        }

        async fn vector_by_id(
            &self,
            _collection_id: &str,
            _base_path: &str,
            _vector_id: &str,
        ) -> Result<Option<ProximaRecord>> {
            Ok(None)
        }

        async fn point_lookup(
            &self,
            collection_id: &str,
            base_path: &str,
            ids: &[String],
            identity: Option<crate::core::stable_id::CollectionIdentity>,
        ) -> Result<Vec<ProximaRecord>> {
            self.calls.lock().await.push((
                collection_id.to_string(),
                base_path.to_string(),
                ids.to_vec(),
                identity,
            ));
            Ok(ids
                .iter()
                .any(|id| id == &self.record.oid)
                .then(|| self.record.clone())
                .into_iter()
                .collect())
        }

        async fn search_vectors_unified(
            &self,
            _ctx: &StorageQueryContext,
        ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
            Ok(Vec::new())
        }
    }

    fn record_with_vector(id: &str, values: Vec<f32>) -> ProximaRecord {
        ProximaRecord {
            oid: id.to_string(),
            embeddings: vec![proximadb_records::EmbeddingCell {
                model_id: "default".to_string(),
                modality: "vector".to_string(),
                dim: values.len() as u32,
                values: proximadb_records::EmbeddingValues::Fp32(values),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    async fn create_test_coordinator() -> Result<(
        VectorReadCoordinator,
        Arc<Mutex<Vec<PointLookupCall>>>,
        TempDir,
    )> {
        let temp_dir = TempDir::new()?;
        let wal_config = crate::storage::persistence::write_ahead_log::WALConfig::default();
        let wal_manager = Arc::new(
            crate::storage::persistence::write_ahead_log::WriteAheadLogManager::new(wal_config)
                .await?,
        );

        let metadata_url = format!("file://{}", temp_dir.path().join("metadata").display());
        let mut config = crate::core::Config::default();
        config.storage.metadata_url = metadata_url.clone();
        let catalog_manager = Arc::new(crate::catalog::CatalogManager::new());
        catalog_manager
            .create_native_catalog("default", &metadata_url)
            .await?;
        let collection_port = Arc::new(
            crate::services::collection::manager::CollectionService::new(config.storage)
                .await?
                .with_catalog_manager(catalog_manager),
        );

        let collection_id = "recovered-helix";
        let base_path = "adls://proximadb/collections";
        let collection_cache = Arc::new(DashMap::new());
        collection_cache.insert(
            collection_id.to_string(),
            Arc::new(crate::proto::proximadb_v1::Collection {
                id: collection_id.to_string(),
                config: Some(crate::proto::proximadb_v1::CollectionConfig {
                    name: collection_id.to_string(),
                    storage_engine: Some(crate::proto::proximadb_v1::StorageEngine::Helix as i32),
                    ..Default::default()
                }),
                storage_assignment: Some(crate::proto::proximadb_v1::StorageAssignment {
                    primary_path: base_path.to_string(),
                    base_location: base_path.to_string(),
                    engine: crate::proto::proximadb_v1::StorageEngine::Helix as i32,
                    typed_account_id: Some(7),
                    typed_namespace_id: Some(11),
                    typed_collection_id: Some(13),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        );

        let calls = Arc::new(Mutex::new(Vec::new()));
        let engine_cache: Arc<DashMap<String, Arc<dyn UnifiedStorageFormat>>> =
            Arc::new(DashMap::new());
        engine_cache.insert(
            collection_id.to_string(),
            Arc::new(PointLookupProbeEngine {
                record: record_with_vector("persisted-record", vec![1.0, 2.0, 3.0]),
                calls: calls.clone(),
            }),
        );

        let resolver = CollectionResolver::new(
            collection_cache,
            engine_cache,
            collection_port,
            wal_manager.clone(),
        );
        Ok((
            VectorReadCoordinator::new(wal_manager, resolver),
            calls,
            temp_dir,
        ))
    }

    #[tokio::test]
    async fn wal_miss_routes_to_collection_engine_and_object_store_base_path() {
        let (coordinator, calls, _temp_dir) = create_test_coordinator().await.unwrap();

        let record = coordinator
            .vector("recovered-helix", "persisted-record", true, true)
            .await
            .unwrap()
            .expect("configured HELIX engine should serve the recovered record");

        assert_eq!(record.oid, "persisted-record");
        assert_eq!(record.embeddings.len(), 1);
        assert_eq!(
            calls.lock().await.as_slice(),
            &[(
                "recovered-helix".to_string(),
                "adls://proximadb/collections".to_string(),
                vec!["persisted-record".to_string()],
                Some(crate::core::stable_id::CollectionIdentity {
                    account_id: 7,
                    namespace_id: 11,
                    collection_id: 13,
                }),
            )]
        );
    }
}
