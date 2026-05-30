//! `DriftWatcher` — the first *live* producer for the F1 trigger arm (T1.9).
//!
//! The three named signal sources (RecallProbeGate, freshness SLA, AutoML) are
//! not yet driven by live data (audited 2026-05-30 — none have production
//! callers). Write volume, however, IS live. This watcher periodically compares
//! a collection's **per-collection** write high-water-mark (the highest global
//! LSN among *its own* manifest entries — not the global allocator, which
//! advances on every collection's writes) against the high-water-mark captured
//! at its last completed recluster and, once the delta crosses a threshold,
//! emits a `WorkloadDrift` signal — closing the flywheel on real data:
//!
//! ```text
//! writes to THIS collection accumulate
//!   -> collection_write_lsn(now) - collection_write_lsn(@last_recluster) >= THRESHOLD
//!   -> DiscoveryService::on_signal(coll, WorkloadDrift)
//!   -> executor runs Recluster -> atomic republish
//!   -> baseline advances -> drift resets
//! ```
//!
//! Per-collection precision (2026-05-30): the watcher reads each collection's
//! own manifest high-water-mark, so a *quiet* collection no longer reclusters
//! just because *other* collections are writing. Considered by name (discovery
//! keys jobs/pins by name); write volume read by internal id (the WAL manifest
//! keys entries by uuid) — the catalog list supplies the name->id mapping.
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
    /// Source of all collection names, swept even before a collection has any
    /// discovery history (closes the bootstrap gap). `None` => history-only.
    /// Collections with no / too-few indexed vectors simply no-op in the pass.
    /// Names (not ids) — the discovery pipeline keys jobs/pins by name.
    collection_source: Option<Arc<dyn proximadb_runtime::CollectionPort>>,
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
            collection_source: None,
        }
    }

    /// Attach a collection source so the watcher sweeps every collection by name
    /// — not just those with prior discovery history. This is what makes the
    /// loop autonomous for brand-new collections (no operator seed needed).
    pub fn with_collection_source(
        mut self,
        source: Arc<dyn proximadb_runtime::CollectionPort>,
    ) -> Self {
        self.collection_source = Some(source);
        self
    }

    /// One sweep. Considers the union of (a) collections with discovery history
    /// and (b) every collection in the catalog (so a never-reclustered
    /// collection is picked up automatically). Each is considered by **name**
    /// (the discovery pipeline keys jobs/pins by name) but its write volume is
    /// read by **internal id** (the WAL manifest keys entries by uuid) — hence
    /// the (name, id) pairing. Returns the number of signals emitted (coalesced
    /// considerations don't count).
    pub async fn tick(&self) -> usize {
        // name -> internal id. The id is used solely for the per-collection
        // write-volume lookup; the catalog list is the authoritative source of
        // the name->id mapping (proto `Collection` carries both).
        let mut by_name: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if let Some(source) = &self.collection_source {
            if let Ok(cols) = source.list_collections(None).await {
                for c in cols {
                    if let Some(name) = c.config.as_ref().map(|cfg| cfg.name.clone()) {
                        by_name.entry(name).or_insert(c.id);
                    }
                }
            }
        }
        // History-only collections may not be in the catalog list; fall back to
        // name-as-id (the manifest lookup then returns 0 — no spurious signal).
        for c in self.service.registry().collections() {
            by_name.entry(c.clone()).or_insert_with(|| c.clone());
        }

        let mut emitted = 0;
        for (name, internal_id) in by_name {
            if self.consider(&name, &internal_id).await.is_some() {
                emitted += 1;
            }
        }
        if emitted > 0 {
            debug!("DriftWatcher: emitted {emitted} recluster signal(s)");
        }
        emitted
    }

    /// Consider one collection: read *this* collection's write high-water-mark
    /// (manifest entries keyed by `internal_id`), compare to its recluster
    /// baseline (keyed by `name`), signal if drifted. A missing manifest /
    /// unknown id degrades to LSN 0 (no spurious signal).
    async fn consider(&self, name: &str, internal_id: &str) -> Option<DiscoveryJob> {
        let current_lsn = self.coordinator.collection_write_lsn(internal_id).await;
        self.consider_with_lsn(name, current_lsn).await
    }

    /// Testable decision core: decide using an explicit current LSN (no manifest
    /// needed). `collection_id` is the user-facing **name** (drift baseline +
    /// `on_signal` key by name). Returns the enqueued job, or `None` if within
    /// threshold or coalesced against an in-flight recluster.
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
        // Baseline is now the per-collection write high-water-mark, not the
        // global snapshot upper bound.
        j.collection_write_lsn = 100;
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
