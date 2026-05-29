//! `DiscoveryService` — control-plane entry point for discovery jobs.

use std::sync::Arc;

use super::job::{DiscoveryJob, DiscoveryJobKind};
use super::registry::DiscoveryRegistry;
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

    /// Look up a job by id.
    pub fn get_job(&self, job_id: &str) -> Option<DiscoveryJob> {
        self.registry.get(job_id)
    }

    /// All jobs for a collection (newest first).
    pub fn list_jobs(&self, collection_id: &str) -> Vec<DiscoveryJob> {
        self.registry.list_for_collection(collection_id)
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
