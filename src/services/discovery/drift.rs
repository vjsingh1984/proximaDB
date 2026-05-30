//! `DriftWatcher` — the first *live* producer for the F1 trigger arm (T1.9).
//!
//! The three named signal sources (RecallProbeGate, freshness SLA, AutoML) are
//! not yet driven by live data (audited 2026-05-30 — none have production
//! callers). Write volume, however, IS live: the global manifest LSN advances on
//! every write. This watcher periodically compares a collection's current LSN
//! against the `snapshot_to_lsn` of its last completed recluster and, once the
//! delta crosses a threshold, emits a `WorkloadDrift` signal — closing the
//! flywheel on real data:
//!
//! ```text
//! writes accumulate -> current_lsn - last_recluster_lsn >= THRESHOLD
//!   -> DiscoveryService::on_signal(coll, WorkloadDrift)
//!   -> executor runs Recluster -> atomic republish
//!   -> baseline (snapshot_to_lsn) advances -> drift resets
//! ```
//!
//! Coalescing (in `DiscoveryTrigger`) prevents a sustained drift from flooding
//! the registry; a completed recluster advances the baseline. AutoML will later
//! subsume this as the cost-aware scheduler.
//!
//! Scope: watches collections that already have discovery history (the registry
//! keys off existing jobs). Recluster is currently non-mutating (it computes
//! cluster-quality metrics + republishes freshness), so an on-by-default watcher
//! is safe even if it fires more often than ideal.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, info};

use super::job::DiscoveryJob;
use super::service::DiscoveryService;
use super::trigger::TriggerSignal;
use crate::services::snapshot::SnapshotPublishCoordinator;

/// Default LSN delta since the last recluster before drift triggers a new one.
/// Conservative starting heuristic; tune per workload (AutoML will subsume).
pub const DEFAULT_DRIFT_THRESHOLD_LSN: u64 = 10_000;
/// Default interval between drift sweeps.
pub const DEFAULT_DRIFT_INTERVAL: Duration = Duration::from_secs(60);

/// Env override for [`DEFAULT_DRIFT_THRESHOLD_LSN`] (used at watcher construction
/// in `SharedServices`). Lets operators tune drift sensitivity, and lets the e2e
/// drive the loop fast without a code change.
pub const DRIFT_THRESHOLD_ENV: &str = "PROXIMADB_DRIFT_THRESHOLD_LSN";
/// Env override (whole seconds) for [`DEFAULT_DRIFT_INTERVAL`].
pub const DRIFT_INTERVAL_ENV: &str = "PROXIMADB_DRIFT_INTERVAL_SECS";

/// Resolve the drift threshold from `PROXIMADB_DRIFT_THRESHOLD_LSN`, else default.
pub fn threshold_lsn_from_env() -> u64 {
    parse_threshold(std::env::var(DRIFT_THRESHOLD_ENV).ok())
}

/// Resolve the drift interval from `PROXIMADB_DRIFT_INTERVAL_SECS`, else default.
pub fn interval_from_env() -> Duration {
    parse_interval(std::env::var(DRIFT_INTERVAL_ENV).ok())
}

/// Pure parse of the threshold override (testable without touching the env).
fn parse_threshold(raw: Option<String>) -> u64 {
    raw.and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_DRIFT_THRESHOLD_LSN)
}

/// Pure parse of the interval override. Non-positive / unparseable => default.
fn parse_interval(raw: Option<String>) -> Duration {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_DRIFT_INTERVAL)
}

/// True when write volume since the last recluster crosses `threshold`.
/// `baseline` is the last completed recluster's `snapshot_to_lsn` (`None` =>
/// never reclustered, baseline 0). A current LSN below the baseline (e.g. clock
/// skew / manifest reset) yields 0 drift, never a spurious signal.
pub fn drift_exceeds(current_lsn: u64, baseline: Option<u64>, threshold: u64) -> bool {
    current_lsn.saturating_sub(baseline.unwrap_or(0)) >= threshold
}

/// Background watcher that turns write-volume drift into recluster signals.
pub struct DriftWatcher {
    service: Arc<DiscoveryService>,
    coordinator: Arc<SnapshotPublishCoordinator>,
    threshold_lsn: u64,
}

impl DriftWatcher {
    pub fn new(
        service: Arc<DiscoveryService>,
        coordinator: Arc<SnapshotPublishCoordinator>,
        threshold_lsn: u64,
    ) -> Self {
        Self {
            service,
            coordinator,
            threshold_lsn,
        }
    }

    /// One sweep over all collections with discovery history. Returns the number
    /// of signals emitted (coalesced considerations don't count).
    pub async fn tick(&self) -> usize {
        let mut emitted = 0;
        for collection_id in self.service.registry().collections() {
            if self.consider(&collection_id).await.is_some() {
                emitted += 1;
            }
        }
        if emitted > 0 {
            debug!("DriftWatcher: emitted {emitted} recluster signal(s)");
        }
        emitted
    }

    /// Consider one collection: read its current LSN (via the snapshot
    /// coordinator's manifest pin), compare to its recluster baseline, signal if
    /// drifted. Pin failure degrades to LSN 0 (no spurious signal).
    async fn consider(&self, collection_id: &str) -> Option<DiscoveryJob> {
        let current_lsn = self
            .coordinator
            .pin(collection_id)
            .await
            .ok()
            .map(|p| p.to_lsn)
            .unwrap_or(0);
        self.consider_with_lsn(collection_id, current_lsn).await
    }

    /// Testable decision core: decide using an explicit current LSN (no manifest
    /// needed). Returns the enqueued job, or `None` if within threshold or
    /// coalesced against an in-flight recluster.
    async fn consider_with_lsn(
        &self,
        collection_id: &str,
        current_lsn: u64,
    ) -> Option<DiscoveryJob> {
        let baseline = self.service.last_reclustered_lsn(collection_id);
        if drift_exceeds(current_lsn, baseline, self.threshold_lsn) {
            self.service
                .on_signal(collection_id, TriggerSignal::WorkloadDrift)
        } else {
            None
        }
    }
}

/// Spawn the background drift-watch loop (mirrors `spawn_discovery_executor`).
pub fn spawn_drift_watcher(
    watcher: Arc<DriftWatcher>,
    mut shutdown: watch::Receiver<bool>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!(
            "DriftWatcher started (interval = {interval:?}, threshold = {} LSN)",
            watcher.threshold_lsn
        );
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(interval) => {
                    watcher.tick().await;
                }
            }
        }
        info!("DriftWatcher stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogManager;
    use crate::services::discovery::{
        DiscoveryJob, DiscoveryJobKind, DiscoveryJobStatus, DiscoveryRegistry,
    };

    #[test]
    fn env_override_parsing() {
        assert_eq!(super::parse_threshold(Some("1".to_string())), 1);
        assert_eq!(super::parse_threshold(Some("  42 ".to_string())), 42);
        assert_eq!(
            super::parse_threshold(None),
            super::DEFAULT_DRIFT_THRESHOLD_LSN
        );
        assert_eq!(
            super::parse_threshold(Some("nope".to_string())),
            super::DEFAULT_DRIFT_THRESHOLD_LSN
        );
        assert_eq!(super::parse_interval(Some("2".to_string())), Duration::from_secs(2));
        // Zero / garbage fall back to the default (never a 0s busy-loop).
        assert_eq!(super::parse_interval(Some("0".to_string())), super::DEFAULT_DRIFT_INTERVAL);
        assert_eq!(super::parse_interval(None), super::DEFAULT_DRIFT_INTERVAL);
    }

    #[test]
    fn drift_threshold_math() {
        assert!(!drift_exceeds(5, Some(0), 10));
        assert!(drift_exceeds(10, Some(0), 10));
        assert!(drift_exceeds(100, None, 10)); // never reclustered, baseline 0
        assert!(!drift_exceeds(105, Some(100), 10)); // only 5 since baseline
        assert!(drift_exceeds(110, Some(100), 10)); // 10 since baseline
        assert!(!drift_exceeds(50, Some(100), 10)); // current < baseline => 0 drift
    }

    async fn watcher() -> (Arc<DriftWatcher>, Arc<DiscoveryRegistry>) {
        // consider_with_lsn never pins, so an empty in-memory catalog suffices.
        let coordinator = Arc::new(SnapshotPublishCoordinator::new(Arc::new(
            CatalogManager::new(),
        )));
        let registry = Arc::new(DiscoveryRegistry::new());
        let service = Arc::new(DiscoveryService::new(
            registry.clone(),
            coordinator.clone(),
        ));
        (
            Arc::new(DriftWatcher::new(service, coordinator, 10)),
            registry,
        )
    }

    #[tokio::test]
    async fn signals_when_never_reclustered_and_drift_exceeds() {
        let (w, _r) = watcher().await;
        assert!(w.consider_with_lsn("c1", 20).await.is_some());
    }

    #[tokio::test]
    async fn respects_last_recluster_baseline() {
        let (w, registry) = watcher().await;
        let mut j = DiscoveryJob::new("c1", DiscoveryJobKind::Recluster);
        j.status = DiscoveryJobStatus::Complete;
        j.snapshot_to_lsn = 100;
        registry.upsert(j);
        // 5 LSN past baseline 100 < threshold 10 -> no signal.
        assert!(w.consider_with_lsn("c1", 105).await.is_none());
        // 15 LSN past baseline -> signal.
        assert!(w.consider_with_lsn("c1", 115).await.is_some());
    }

    #[tokio::test]
    async fn sustained_drift_is_coalesced_while_recluster_in_flight() {
        let (w, _r) = watcher().await;
        assert!(w.consider_with_lsn("c1", 50).await.is_some(), "first drift enqueues");
        assert!(
            w.consider_with_lsn("c1", 60).await.is_none(),
            "still drifted but a recluster is in flight -> coalesced"
        );
    }
}
