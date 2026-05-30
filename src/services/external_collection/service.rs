//! `ExternalCollectionService` — register / build / search external collections
//! (Phase 8 F5 / TD-090).
//!
//! Register catalogs the external table un-copied (`FederatedRead` authority +
//! a `VectorAnn` projection in `ExternalSnapshotRegistered`). Build reads the
//! external source, constructs a ProximaDB-owned IVF index **in place** (via
//! `AxisManager::rebuild_and_swap_ivf_index`, which writes nothing to WAL or
//! storage), registers the IVF routing strategy, and flips the projection to
//! `Fresh` with the snapshot fingerprint as lineage. Search delegates to the
//! same `AxisManager::query` path native collections use.

use std::sync::Arc;

use anyhow::{Context, Result};
use proximadb_catalog::{
    CatalogColumn, CatalogDataType, CatalogProjection, CatalogProjectionKind, CatalogStorageLayout,
    CatalogTableSchema, ProjectionFreshnessState,
};

use super::registry::ExternalCollectionRegistry;
use super::source_reader::{read_external_records, snapshot_fingerprint};
use super::types::{ExternalCollection, ExternalCollectionSpec, ExternalCollectionStatus};
use crate::catalog::CatalogManager;
use crate::index::axis::management::manager::{AxisHybridQuery, VectorQuery};
use crate::index::axis::types::{Data, IndexAlgorithm, IndexSelectionStrategy, IndexSpecification};
use crate::index::AxisManager;

/// Canonical name of the projection that tracks the ProximaDB-owned vector
/// index built over the external source.
pub const EXTERNAL_INDEX_PROJECTION: &str = "external_index";

/// Control-plane facade for external collections.
pub struct ExternalCollectionService {
    registry: Arc<ExternalCollectionRegistry>,
    catalog_manager: Arc<CatalogManager>,
    axis_manager: Arc<AxisManager>,
}

impl ExternalCollectionService {
    pub fn new(
        registry: Arc<ExternalCollectionRegistry>,
        catalog_manager: Arc<CatalogManager>,
        axis_manager: Arc<AxisManager>,
    ) -> Self {
        Self {
            registry,
            catalog_manager,
            axis_manager,
        }
    }

    /// Shared registry handle.
    pub fn registry(&self) -> Arc<ExternalCollectionRegistry> {
        self.registry.clone()
    }

    /// Register an external collection un-copied: catalog it with `FederatedRead`
    /// authority + a `VectorAnn` projection in `ExternalSnapshotRegistered`, and
    /// record it in the durable registry. Does **not** read or copy the source.
    pub async fn register(&self, spec: ExternalCollectionSpec) -> Result<ExternalCollection> {
        if self.registry.get_by_name(&spec.name).is_some() {
            anyhow::bail!("external collection '{}' is already registered", spec.name);
        }
        let snapshot_id = snapshot_fingerprint(&spec.location)
            .with_context(|| format!("fingerprint external source '{}'", spec.location))?;

        let (catalog, identifier) = self.catalog_manager.resolve_table(&spec.name).await?;
        if catalog.table_exists(&identifier).await? {
            anyhow::bail!(
                "a catalog table named '{}' already exists; cannot register as external",
                spec.name
            );
        }
        // Ensure the namespace exists (idempotent — ignore "already exists").
        let _ = catalog
            .create_namespace(&identifier.namespace, std::collections::HashMap::new())
            .await;

        let layout = CatalogStorageLayout::federated_read(
            "external_source",
            spec.format.catalog_format(),
            spec.location.clone(),
        );
        let projection = CatalogProjection::rebuildable(
            EXTERNAL_INDEX_PROJECTION,
            CatalogProjectionKind::VectorAnn,
            &spec.name,
        )
        .with_freshness_state(ProjectionFreshnessState::ExternalSnapshotRegistered)
        .with_lineage(snapshot_id.clone(), snapshot_id.clone());

        // A `Vector` catalog column must declare its `dimension` property
        // (enforced by `validate_storage_contract`); `CatalogColumn` exposes the
        // property map directly (no builder method).
        let mut vector_col = CatalogColumn::new(1, &spec.vector_column, CatalogDataType::Vector);
        vector_col
            .properties
            .insert("dimension".to_string(), spec.dimension.to_string());
        let mut schema = CatalogTableSchema::new(spec.name.clone())
            .with_column(CatalogColumn::new(0, &spec.id_column, CatalogDataType::String))
            .with_column(vector_col)
            .with_projection(projection);
        // `CatalogTableSchema::default()` seeds one `InternalCanonical` layout;
        // an external collection has no internal-canonical storage, so replace
        // it with the federated-read layout (rather than appending behind it).
        schema.storage_layouts = vec![layout];
        catalog
            .create_table(&identifier, schema)
            .await
            .with_context(|| format!("catalog external collection '{}'", spec.name))?;

        let ec = ExternalCollection::new(spec, snapshot_id);
        self.registry.upsert(ec.clone());
        Ok(ec)
    }

    /// Build the ProximaDB-owned IVF index in place over the external source and
    /// publish it (`Fresh`). Reads the source but copies nothing into ProximaDB
    /// storage. Returns the number of records indexed.
    pub async fn build(&self, id: &str) -> Result<usize> {
        let mut ec = self
            .registry
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("external collection '{id}' not found"))?;
        let collection_id = ec.spec.name.clone();

        ec.status = ExternalCollectionStatus::Building;
        self.registry.upsert(ec.clone());

        match self.build_inner(&ec).await {
            Ok(count) => {
                ec.status = ExternalCollectionStatus::Ready;
                ec.indexed_record_count = count as u64;
                ec.error = None;
                self.registry.upsert(ec);
                self.set_projection_state(&collection_id, |p| {
                    p.freshness_state = ProjectionFreshnessState::Fresh;
                })
                .await?;
                Ok(count)
            }
            Err(err) => {
                let msg = format!("{err:#}");
                ec.status = ExternalCollectionStatus::Failed;
                ec.error = Some(msg.clone());
                self.registry.upsert(ec);
                // Best-effort: mark the projection unusable so planners skip it.
                let _ = self
                    .set_projection_state(&collection_id, |p| {
                        p.freshness_state = ProjectionFreshnessState::RebuildRequired;
                    })
                    .await;
                Err(err)
            }
        }
    }

    /// Read + build + register-strategy. Separated so `build` can centralize
    /// status/projection transitions around it.
    async fn build_inner(&self, ec: &ExternalCollection) -> Result<usize> {
        let records = read_external_records(&ec.spec)?;
        let count = records.len();
        let built = self
            .axis_manager
            .rebuild_and_swap_ivf_index(&ec.spec.name, &records)
            .await?;
        if !built {
            anyhow::bail!(
                "external collection '{}': too few vectors to build an IVF index ({} read)",
                ec.spec.name,
                count
            );
        }
        self.register_ivf_strategy(&ec.spec.name, ec.spec.dimension, count)
            .await?;
        Ok(count)
    }

    /// Register the IVF routing strategy so `AxisManager::query` routes this
    /// collection to its (already-built) IVF index. The nlist/nprobe here are
    /// routing hints; the served index keeps the parameters it was built with.
    async fn register_ivf_strategy(
        &self,
        collection_id: &str,
        dimension: usize,
        n: usize,
    ) -> Result<()> {
        let nlist = ((n as f32).sqrt() as usize * 2).clamp(16, 256) as u32;
        let nprobe = (nlist / 2).max(1);
        let strategy = IndexSelectionStrategy {
            indexes: vec![IndexSpecification::new(
                Data::DenseVector { dimension },
                IndexAlgorithm::IVF {
                    nlist,
                    nprobe,
                    quantizer: None,
                },
            )],
            routing_rules: vec![],
        };
        self.axis_manager
            .update_collection_strategy(collection_id, strategy)
            .await
    }

    /// Search the external collection's index, returning `(id, score)` pairs.
    /// Delegates to the same `AxisManager::query` path native collections use.
    pub async fn search(
        &self,
        id: &str,
        query: Vec<f32>,
        k: usize,
    ) -> Result<Vec<(String, f32)>> {
        let ec = self
            .registry
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("external collection '{id}' not found"))?;
        if !ec.is_ready() {
            anyhow::bail!(
                "external collection '{}' is not ready (status {:?}); build it first",
                ec.spec.name,
                ec.status
            );
        }
        let q = AxisHybridQuery {
            collection_id: ec.spec.name.clone(),
            vector_query: Some(VectorQuery::Dense {
                vector: query,
                similarity_threshold: 0.0,
            }),
            top_k: k,
            ..AxisHybridQuery::default()
        };
        let result = self.axis_manager.query(q).await?;
        Ok(result
            .results
            .into_iter()
            .map(|r| (r.vector_id, r.similarity))
            .collect())
    }

    /// Look up a registry record by id.
    pub fn get(&self, id: &str) -> Option<ExternalCollection> {
        self.registry.get(id)
    }

    /// All registered external collections (newest first).
    pub fn list(&self) -> Vec<ExternalCollection> {
        self.registry.list_all()
    }

    /// Read the external-index projection (for EXPLAIN / route-health / tests).
    pub async fn projection(&self, collection_id: &str) -> Result<Option<CatalogProjection>> {
        let (catalog, identifier) = self.catalog_manager.resolve_table(collection_id).await?;
        if !catalog.table_exists(&identifier).await? {
            return Ok(None);
        }
        let schema = catalog.get_table(&identifier).await?;
        Ok(schema
            .projections
            .into_iter()
            .find(|p| p.name == EXTERNAL_INDEX_PROJECTION))
    }

    /// Mutate the external-index projection via the drop+create update pattern
    /// (no `Catalog::update_table` exists), mirroring `SnapshotPublishCoordinator`.
    async fn set_projection_state<F>(&self, collection_id: &str, mutate: F) -> Result<()>
    where
        F: FnOnce(&mut CatalogProjection),
    {
        let (catalog, identifier) = self.catalog_manager.resolve_table(collection_id).await?;
        if !catalog.table_exists(&identifier).await? {
            anyhow::bail!("external collection '{collection_id}' is not cataloged");
        }
        let mut schema = catalog.get_table(&identifier).await?;
        match schema
            .projections
            .iter_mut()
            .find(|p| p.name == EXTERNAL_INDEX_PROJECTION)
        {
            Some(proj) => mutate(proj),
            None => {
                let mut proj = CatalogProjection::rebuildable(
                    EXTERNAL_INDEX_PROJECTION,
                    CatalogProjectionKind::VectorAnn,
                    collection_id,
                );
                mutate(&mut proj);
                schema.projections.push(proj);
            }
        }
        catalog.drop_table(&identifier, false).await?;
        catalog.create_table(&identifier, schema).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, StringBuilder};
    use arrow_array::RecordBatch;
    use arrow_schema::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;

    async fn catalog_manager_with_default() -> Arc<CatalogManager> {
        let tmp = std::env::temp_dir().join(format!(
            "proximadb_extsvc_cat_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("default", &format!("file://{}", tmp.display()))
            .await
            .unwrap();
        manager.set_default_catalog("default").await.unwrap();
        manager
    }

    async fn axis_manager() -> Arc<AxisManager> {
        Arc::new(
            AxisManager::new(crate::index::AxisConfig::default())
                .await
                .unwrap(),
        )
    }

    /// Write `n` vectors with **unique directions** (one-hot at position `i`,
    /// so under cosine each row's nearest neighbor is itself). Requires
    /// `dim >= n`. Rebuild needs >= 16 vectors to cluster.
    fn write_parquet_fixture(path: &std::path::Path, n: usize, dim: usize) -> Vec<(String, Vec<f32>)> {
        assert!(dim >= n, "one-hot fixture requires dim >= n");
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dim as i32,
                ),
                false,
            ),
        ]));
        let mut id_b = StringBuilder::new();
        let mut vec_b = FixedSizeListBuilder::new(Float32Builder::new(), dim as i32);
        let mut expect = Vec::new();
        for i in 0..n {
            let id = format!("row-{i}");
            let mut v = vec![0.0f32; dim];
            v[i] = 1.0 + (i as f32) * 0.001;
            id_b.append_value(&id);
            for x in &v {
                vec_b.values().append_value(*x);
            }
            vec_b.append(true);
            expect.push((id, v));
        }
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(id_b.finish()), Arc::new(vec_b.finish())],
        )
        .unwrap();
        let file = std::fs::File::create(path).unwrap();
        let mut writer = ArrowWriter::try_new(file, schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        expect
    }

    fn fixture_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "proximadb_extsvc_{}.parquet",
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[tokio::test]
    async fn register_catalogs_federated_read_and_external_snapshot() {
        let path = fixture_path();
        write_parquet_fixture(&path, 20, 20);
        let cat = catalog_manager_with_default().await;
        let svc = ExternalCollectionService::new(
            Arc::new(ExternalCollectionRegistry::new()),
            cat.clone(),
            axis_manager().await,
        );

        let spec =
            ExternalCollectionSpec::parquet("ext_docs", path.to_str().unwrap(), "id", "vector", 20);
        let ec = svc.register(spec).await.unwrap();
        assert_eq!(ec.status, ExternalCollectionStatus::Registered);

        // Catalog shows FederatedRead authority + external location.
        let (catalog, identifier) = cat.resolve_table("ext_docs").await.unwrap();
        let schema = catalog.get_table(&identifier).await.unwrap();
        let layout = &schema.storage_layouts[0];
        assert_eq!(
            layout.authority,
            proximadb_catalog::CatalogAuthorityMode::FederatedRead
        );
        assert_eq!(layout.location.as_deref(), Some(path.to_str().unwrap()));

        // Projection registered as ExternalSnapshotRegistered with snapshot lineage.
        let proj = svc.projection("ext_docs").await.unwrap().unwrap();
        assert_eq!(
            proj.freshness_state,
            ProjectionFreshnessState::ExternalSnapshotRegistered
        );
        assert_eq!(proj.source_range.as_deref(), Some(ec.snapshot_id.as_str()));

        // Re-register is rejected.
        let dup =
            ExternalCollectionSpec::parquet("ext_docs", path.to_str().unwrap(), "id", "vector", 20);
        assert!(svc.register(dup).await.is_err());

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn build_in_place_makes_index_queryable_and_fresh() {
        let path = fixture_path();
        let expect = write_parquet_fixture(&path, 32, 32);
        let cat = catalog_manager_with_default().await;
        let axis = axis_manager().await;
        let svc = ExternalCollectionService::new(
            Arc::new(ExternalCollectionRegistry::new()),
            cat.clone(),
            axis.clone(),
        );

        let spec =
            ExternalCollectionSpec::parquet("ext_docs", path.to_str().unwrap(), "id", "vector", 32);
        let ec = svc.register(spec).await.unwrap();

        let count = svc.build(&ec.id).await.unwrap();
        assert_eq!(count, 32);
        assert!(axis.has_ivf_index("ext_docs").await);

        // Status Ready + projection Fresh.
        let got = svc.get(&ec.id).unwrap();
        assert_eq!(got.status, ExternalCollectionStatus::Ready);
        assert_eq!(got.indexed_record_count, 32);
        let proj = svc.projection("ext_docs").await.unwrap().unwrap();
        assert_eq!(proj.freshness_state, ProjectionFreshnessState::Fresh);

        // Search returns the exact nearest external row for a row's own vector.
        let (qid, qvec) = &expect[5];
        let hits = svc.search(&ec.id, qvec.clone(), 3).await.unwrap();
        assert!(!hits.is_empty(), "external index must be queryable");
        assert_eq!(&hits[0].0, qid, "top-1 must be the exact external row");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn search_before_build_is_rejected() {
        let path = fixture_path();
        write_parquet_fixture(&path, 20, 20);
        let cat = catalog_manager_with_default().await;
        let svc = ExternalCollectionService::new(
            Arc::new(ExternalCollectionRegistry::new()),
            cat,
            axis_manager().await,
        );
        let spec =
            ExternalCollectionSpec::parquet("ext_docs", path.to_str().unwrap(), "id", "vector", 20);
        let ec = svc.register(spec).await.unwrap();
        assert!(svc.search(&ec.id, vec![0.0; 20], 3).await.is_err());
        let _ = std::fs::remove_file(&path);
    }
}
