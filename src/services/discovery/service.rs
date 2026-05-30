//! `DiscoveryService` — control-plane entry point for discovery jobs.

use std::sync::Arc;

use super::job::{DiscoveryJob, DiscoveryJobKind, DiscoveryJobStatus};
use super::registry::DiscoveryRegistry;
use super::trigger::{DiscoveryTrigger, TriggerSignal};
use crate::services::snapshot::SnapshotPublishCoordinator;

/// Control-plane facade: create / inspect discovery jobs. The background
/// `DiscoveryJobExecutor` consumes scheduled jobs from the same registry.
pub struct DiscoveryService {
    registry: Arc<DiscoveryRegistry>,
    coordinator: Arc<SnapshotPublishCoordinator>,
}

impl DiscoveryService {
    pub fn new(
        registry: Arc<DiscoveryRegistry>,
        coordinator: Arc<SnapshotPublishCoordinator>,
    ) -> Self {
        Self {
            registry,
            coordinator,
        }
    }

    /// Create and schedule a discovery job for a collection.
    pub fn create_job(
        &self,
        collection_id: impl Into<String>,
        kind: DiscoveryJobKind,
    ) -> DiscoveryJob {
        self.registry
            .schedule(DiscoveryJob::new(collection_id, kind))
    }

    /// Feedback arm: turn a serving-side signal into a scheduled discovery job,
    /// coalescing against any in-flight job of the same kind for the collection.
    /// Signal sources (RecallProbeGate, freshness state machine, AutoML drift)
    /// call this; returns the enqueued job, or `None` if coalesced.
    pub fn on_signal(
        &self,
        collection_id: &str,
        signal: TriggerSignal,
    ) -> Option<DiscoveryJob> {
        DiscoveryTrigger::new(self.registry.clone()).on_signal(collection_id, signal)
    }

    /// Look up a job by id.
    pub fn get_job(&self, job_id: &str) -> Option<DiscoveryJob> {
        self.registry.get(job_id)
    }

    /// All jobs for a collection (newest first).
    pub fn list_jobs(&self, collection_id: &str) -> Vec<DiscoveryJob> {
        self.registry.list_for_collection(collection_id)
    }

    /// The `snapshot_to_lsn` of the collection's most recent *completed*
    /// `Recluster` job, or `None` if it has never been reclustered. This is the
    /// drift watcher's baseline — write volume past it measures staleness.
    pub fn last_reclustered_lsn(&self, collection_id: &str) -> Option<u64> {
        self.registry
            .list_for_collection(collection_id)
            .into_iter()
            .find(|j| {
                j.kind == DiscoveryJobKind::Recluster
                    && j.status == DiscoveryJobStatus::Complete
            })
            .map(|j| j.snapshot_to_lsn)
    }

    /// Shared registry handle (used to construct the executor).
    pub fn registry(&self) -> Arc<DiscoveryRegistry> {
        self.registry.clone()
    }

    /// Shared snapshot coordinator handle (used to construct the executor and
    /// to surface discovery freshness in route-health / EXPLAIN).
    pub fn coordinator(&self) -> Arc<SnapshotPublishCoordinator> {
        self.coordinator.clone()
    }
}
