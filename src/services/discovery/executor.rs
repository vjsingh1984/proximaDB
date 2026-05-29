//! `DiscoveryJobExecutor` — background worker that drives the CS/CD loop.
//!
//! For each claimed job: pin a snapshot, mark the discovery projection
//! `Updating`, run the refinement pass, then atomically `commit_publish`
//! (`Fresh`) — or `abort_publish` (`RebuildRequired`) on failure. The spawn
//! pattern mirrors `start_axis_consumer` / the embedding drainer: a tokio task
//! with a `watch` shutdown channel.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{error, info, warn};

use super::job::{now_ms, DiscoveryJob, DiscoveryJobKind, DiscoveryJobResult, DiscoveryJobStatus};
use super::registry::DiscoveryRegistry;
use crate::services::snapshot::SnapshotPublishCoordinator;

/// Default poll interval for the background loop.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub struct DiscoveryJobExecutor {
    registry: Arc<DiscoveryRegistry>,
    coordinator: Arc<SnapshotPublishCoordinator>,
}

impl DiscoveryJobExecutor {
    pub fn new(
        registry: Arc<DiscoveryRegistry>,
        coordinator: Arc<SnapshotPublishCoordinator>,
    ) -> Self {
        Self {
            registry,
            coordinator,
        }
    }

    /// Claim and process one scheduled job, if any. Returns the processed job id.
    pub async fn run_once(&self) -> Option<String> {
        let job = self.registry.claim_next_scheduled()?;
        let job_id = job.job_id.clone();
        if let Err(err) = self.execute(job).await {
            error!("discovery job {job_id} failed: {err:#}");
        }
        Some(job_id)
    }

    async fn execute(&self, mut job: DiscoveryJob) -> anyhow::Result<()> {
        // 1. Pin a read-only snapshot of the collection's canonical state.
        let pin = self.coordinator.pin(&job.collection_id).await?;
        job.snapshot_from_lsn = pin.from_lsn;
        job.snapshot_to_lsn = pin.to_lsn;
        job.checkpoint_id = pin.checkpoint_id;
        self.registry.upsert(job.clone());

        // 2. Mark the discovery projection Updating (serving still reads prior).
        if let Err(err) = self.coordinator.begin_publish(&pin).await {
            self.fail(&mut job, format!("begin_publish: {err:#}"));
            return Err(err);
        }

        // 3. Run the refinement pass against the pinned snapshot.
        let result = match self.run_pass(&job).await {
            Ok(result) => result,
            Err(err) => {
                let _ = self.coordinator.abort_publish(&pin).await;
                self.fail(&mut job, format!("pass: {err:#}"));
                return Err(err);
            }
        };

        // 4. Atomic serving switch: commit the republished snapshot (Fresh).
        if let Err(err) = self.coordinator.commit_publish(&pin).await {
            let _ = self.coordinator.abort_publish(&pin).await;
            self.fail(&mut job, format!("commit_publish: {err:#}"));
            return Err(err);
        }

        // 5. Fold the result into the job record and mark Complete.
        job.status = DiscoveryJobStatus::Complete;
        job.completed_at_ms = Some(now_ms());
        job.input_record_count = result.input_record_count;
        job.refined_record_count = result.refined_record_count;
        job.removed_count = result.removed_count;
        job.quality_metrics = result.quality_metrics;
        self.registry.upsert(job);
        Ok(())
    }

    /// Run the refinement pass for a job's kind.
    ///
    /// Walking-skeleton behavior (this increment): every kind is an identity
    /// pass that republishes the pinned snapshot unchanged, proving the
    /// pin -> publish substrate end-to-end. S3 replaces `Dedup` with a real
    /// pass (read pinned records, drop near-duplicates, rewrite via the v2
    /// canonical path); the other kinds remain stubs for later phases.
    async fn run_pass(&self, _job: &DiscoveryJob) -> anyhow::Result<DiscoveryJobResult> {
        match _job.kind {
            DiscoveryJobKind::Dedup
            | DiscoveryJobKind::Recluster
            | DiscoveryJobKind::ReEmbed
            | DiscoveryJobKind::QualityScan
            | DiscoveryJobKind::TrajectoryAnalysis => Ok(DiscoveryJobResult::default()),
        }
    }

    fn fail(&self, job: &mut DiscoveryJob, msg: String) {
        warn!("discovery job {} aborted: {}", job.job_id, msg);
        job.status = DiscoveryJobStatus::Failed;
        job.error = Some(msg);
        job.completed_at_ms = Some(now_ms());
        self.registry.upsert(job.clone());
    }
}

/// Spawn the background executor loop. Drains all currently-scheduled jobs each
/// tick, until the shutdown signal flips to `true` (or the sender is dropped).
pub fn spawn_discovery_executor(
    executor: Arc<DiscoveryJobExecutor>,
    mut shutdown: watch::Receiver<bool>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("DiscoveryJobExecutor started (poll interval = {interval:?})");
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(interval) => {
                    while executor.run_once().await.is_some() {}
                }
            }
        }
        info!("DiscoveryJobExecutor stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogManager;
    use proximadb_catalog::{CatalogColumn, CatalogDataType, CatalogTableSchema, TableIdentifier};

    async fn wired(collection: &str) -> (Arc<DiscoveryJobExecutor>, Arc<SnapshotPublishCoordinator>, Arc<DiscoveryRegistry>) {
        let tmp = std::env::temp_dir().join(format!(
            "proximadb_discovery_exec_{}_{}",
            collection,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("default", &format!("file://{}", tmp.display()))
            .await
            .unwrap();
        manager.set_default_catalog("default").await.unwrap();
        let catalog = manager.default_catalog().await.unwrap();
        let identifier = TableIdentifier::new(vec!["default".to_string()], collection.to_string());
        catalog
            .create_namespace(&identifier.namespace, std::collections::HashMap::new())
            .await
            .unwrap();
        catalog
            .create_table(
                &identifier,
                CatalogTableSchema::new(collection.to_string())
                    .with_column(CatalogColumn::new(0, "oid", CatalogDataType::String)),
            )
            .await
            .unwrap();

        let coordinator = Arc::new(SnapshotPublishCoordinator::new(manager));
        let registry = Arc::new(DiscoveryRegistry::new());
        let executor = Arc::new(DiscoveryJobExecutor::new(registry.clone(), coordinator.clone()));
        (executor, coordinator, registry)
    }

    #[tokio::test]
    async fn identity_job_completes_and_publishes_fresh_snapshot() {
        let (executor, coordinator, registry) = wired("c_exec").await;
        let job = registry.schedule(DiscoveryJob::new("c_exec", DiscoveryJobKind::Dedup));

        let processed = executor.run_once().await;
        assert_eq!(processed.as_deref(), Some(job.job_id.as_str()));

        let done = registry.get(&job.job_id).unwrap();
        assert_eq!(done.status, DiscoveryJobStatus::Complete);
        assert!(done.completed_at_ms.is_some());

        // Atomic republish landed: discovery projection is Fresh with lineage.
        let proj = coordinator.active_projection("c_exec").await.unwrap().unwrap();
        assert_eq!(
            proj.freshness_state,
            proximadb_catalog::ProjectionFreshnessState::Fresh
        );
        assert!(proj.source_range.is_some());
    }

    #[tokio::test]
    async fn job_against_uncataloged_collection_is_marked_failed() {
        let (executor, _coordinator, registry) = wired("c_real").await;
        let job = registry.schedule(DiscoveryJob::new("c_missing", DiscoveryJobKind::Dedup));
        executor.run_once().await;
        let done = registry.get(&job.job_id).unwrap();
        assert_eq!(done.status, DiscoveryJobStatus::Failed);
        assert!(done.error.is_some());
    }
}
