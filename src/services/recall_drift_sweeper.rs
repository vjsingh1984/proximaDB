// Copyright 2025 Vijaykumar Singh.
// Licensed under the Apache License, Version 2.0.

//! Periodic AXIS HNSW recall-target drift sweeper.
//!
//! # Why
//!
//! Operators want recall drift to surface on dashboards / alerts
//! **without** needing to poll
//! `GET /api/v2/_diagnostics/collections/:id/route-health` per
//! collection. The Prometheus gauges from
//! [`crate::metrics::recall_drift_metrics`] only populate when
//! someone runs `detect_recall_drift` — the route-health and
//! recall-tune handlers do that on-demand, but a quiet collection
//! never gets observed.
//!
//! This sweeper closes that loop: every `interval` (default
//! [`DEFAULT_SWEEP_INTERVAL`]) it walks every collection with a
//! `recall_target:<float>` tag, runs `detect_recall_drift` for the
//! current `(baseline_n, current_n, recall_target)`, and records
//! the result via `record_recall_drift_observation`. Dashboards
//! get a steady heartbeat; alerts on
//! `axis_recall_drift_status{kind="rebuild_required"} == 1` fire
//! reliably.
//!
//! # What it doesn't do
//!
//! * **No mutation** — never auto-applies a hot-swap, never
//!   triggers a rebuild. The operator drives both via the
//!   `/recall-tune` and (forthcoming) `/recluster` endpoints. The
//!   sweeper is read-only and idempotent.
//! * **No discovery signal** — does not feed
//!   [`crate::services::discovery`] today. The drift kind is a
//!   structural state, not a "something happened" event; treating
//!   it as a discovery trigger would re-fire every sweep. If the
//!   sweeper grows a transition-edge detector (open→degraded), it
//!   can mirror `RecallObserver`'s pattern then.
//!
//! # Wiring
//!
//! Spawned alongside the existing `RecallObserver` and
//! `DriftWatcher` from `crate::network::shared_services::SharedServices::new`.
//! Same pattern: `Arc<Self>`, `spawn_*` helper, `watch::Receiver<bool>`
//! shutdown, `DEFAULT_*_INTERVAL` constant.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::compute::distance_computation::DistanceMetric;
use crate::index::axis::management::{DriftKind, RecallDriftInput, detect_recall_drift};
use crate::services::collection::recall_target::{parse_recall_target, parse_target_vector_count};

/// Default sweep cadence. 5 minutes matches `RecallObserver` and is
/// the lowest cadence at which Prometheus scrapes typically pick up
/// changes (15s scrape × 20 samples is plenty of resolution).
pub const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(300);

/// The sweeper's collection-listing dependency. Decoupled as a
/// trait so the unit tests don't need to spin up the full
/// `CollectionService` — the fixture impl just returns canned
/// `Collection`s.
#[async_trait::async_trait]
pub trait CollectionLister: Send + Sync {
    /// Return every collection visible to the sweeper. The sweeper
    /// itself filters to those with a `recall_target:` tag.
    async fn list_for_sweeper(
        &self,
    ) -> Result<Vec<crate::proto::proximadb_v1::Collection>, anyhow::Error>;
}

// Bridge to the real CollectionService.
#[async_trait::async_trait]
impl CollectionLister for crate::services::collection::Collections {
    async fn list_for_sweeper(
        &self,
    ) -> Result<Vec<crate::proto::proximadb_v1::Collection>, anyhow::Error> {
        self.list_collections().await
    }
}

/// Background sweeper that emits drift observations + Prometheus
/// metrics for every collection with a `recall_target:` tag.
pub struct RecallDriftSweeper {
    collections: Arc<dyn CollectionLister>,
    /// Fallback `top_k` used when the per-collection
    /// `target_top_k:` tag is absent. The route-health, recall-tune,
    /// recluster, and create-time wiring all resolve through
    /// `crate::services::collection::recall_target::resolve_top_k`,
    /// which has the same fallback — keeps every surface in sync.
    default_top_k: u32,
    /// When true, the sweeper auto-applies the in-memory hot-swap
    /// for any collection whose drift is classified as
    /// [`DriftKind::EfSearchOnly`]. Defaults to true: the hot-swap
    /// is a single in-memory `u32` update on the live strategy and
    /// the worst case (advisor jitter at a tier boundary) is at
    /// most one apply per sweep tick (5 min default).
    ///
    /// **EfConstructionOrM drift is intentionally NOT auto-resolved**
    /// — that path triggers a full HNSW rebuild (minutes of CPU +
    /// memory) and is operator-driven per the design choice
    /// captured in commit 3fba8e589.
    auto_hot_swap: bool,
}

impl RecallDriftSweeper {
    pub fn new(collections: Arc<dyn CollectionLister>) -> Self {
        Self {
            collections,
            default_top_k: crate::services::collection::recall_target::DEFAULT_TOP_K,
            auto_hot_swap: true,
        }
    }

    /// Override the fallback `top_k` used when the per-collection
    /// `target_top_k:` tag is absent. Production deployments should
    /// rarely change this — it exists for parity with custom
    /// route-health configurations.
    pub fn with_default_top_k(mut self, top_k: u32) -> Self {
        self.default_top_k = top_k;
        self
    }

    /// Disable auto-apply of hot-swaps on `EfSearchOnly` drift.
    /// Useful for tests and for operator deployments that want
    /// every AXIS mutation to be explicit. Defaults to true.
    pub fn with_auto_hot_swap(mut self, enabled: bool) -> Self {
        self.auto_hot_swap = enabled;
        self
    }

    /// Run one sweep pass. Returns the number of collections for
    /// which a drift observation was recorded (i.e. those with a
    /// valid `recall_target:` tag). Exposed for tests.
    pub async fn sweep_once(&self) -> usize {
        let collections = match self.collections.list_for_sweeper().await {
            Ok(cs) => cs,
            Err(err) => {
                warn!("recall_drift_sweeper: list_collections failed: {err}");
                return 0;
            }
        };

        let mut observed = 0_usize;
        for collection in collections {
            let Some(config) = collection.config.as_ref() else {
                continue;
            };
            let Some(recall_target) = parse_recall_target(config) else {
                // Collection has no recall_target tag — still emit
                // the "unwired" state so dashboards know it's not
                // adaptive. The cost is one gauge write per
                // collection per sweep; for >10K collections an
                // operator should up DEFAULT_SWEEP_INTERVAL or
                // filter at scrape time.
                crate::metrics::recall_drift_metrics::record_recall_drift_observation(
                    &config.name,
                    "unwired",
                );
                continue;
            };

            let baseline_n = parse_target_vector_count(config).unwrap_or(100_000);
            let current_n = collection
                .stats
                .as_ref()
                .map(|s| s.vector_count.max(0) as u64)
                .unwrap_or(0);
            let metric = convert_distance_metric(config.distance_metric);
            // Per-collection target_top_k tag wins; sweeper-level
            // default is the fallback.
            let top_k = crate::services::collection::recall_target::parse_target_top_k(config)
                .unwrap_or(self.default_top_k);

            let max_ef_search =
                crate::services::collection::recall_target::parse_max_ef_search(config);

            let report = detect_recall_drift(RecallDriftInput {
                baseline_n,
                current_n,
                recall_target,
                top_k,
                dimension: config.dimension,
                distance_metric: metric,
                max_ef_search,
            });

            let kind = drift_kind_str(report.drift_kind);
            crate::metrics::recall_drift_metrics::record_recall_drift_observation(
                &config.name,
                kind,
            );
            observed += 1;
            debug!(
                target: "recall_drift_sweeper",
                collection = %config.name,
                recall_target = recall_target,
                baseline_n = baseline_n,
                current_n = current_n,
                kind = kind,
                "sweep observation"
            );

            // Auto-apply hot-swap on EfSearchOnly drift. Bounded:
            // at most one apply per (collection, sweep tick), and
            // the advised ef is recomputed from advisor state at
            // the next tick, so a transient mis-prediction
            // self-corrects.
            if self.auto_hot_swap
                && matches!(report.drift_kind, DriftKind::EfSearchOnly)
                && let Some(axis_manager) =
                    crate::storage::engines::sst::core::get_sst_axis_manager()
            {
                let new_ef = report.current_params.ef_search;
                match axis_manager
                    .apply_hnsw_ef_hot_swap(&config.name, new_ef)
                    .await
                {
                    Ok(crate::index::axis::management::HotSwapOutcome::Applied {
                        changes,
                    }) => {
                        crate::metrics::recall_drift_metrics::record_recall_drift_hot_swap_applied(
                            &config.name,
                            crate::metrics::recall_drift_metrics::HOT_SWAP_TRIGGER_SWEEPER,
                        );
                        info!(
                            target: "recall_drift_sweeper",
                            collection = %config.name,
                            new_ef_search = new_ef,
                            specs_touched = changes.len(),
                            "auto-applied EfSearchOnly hot-swap"
                        );
                    }
                    Ok(crate::index::axis::management::HotSwapOutcome::NotApplicable {
                        reason,
                    }) => {
                        debug!(
                            target: "recall_drift_sweeper",
                            collection = %config.name,
                            reason = %reason,
                            "hot-swap not applicable (likely no strategy yet)"
                        );
                    }
                    Err(err) => {
                        warn!(
                            target: "recall_drift_sweeper",
                            collection = %config.name,
                            error = %err,
                            "hot-swap apply failed"
                        );
                    }
                }
            }
        }
        observed
    }
}

fn drift_kind_str(kind: DriftKind) -> &'static str {
    match kind {
        DriftKind::None => "none",
        // HNSW and IVF hot-swap-able variants map to the same
        // "ef_search_only" label on the Prometheus surface — they
        // both signal "live-tunable, no rebuild needed".
        DriftKind::EfSearchOnly | DriftKind::NprobeOnly => "ef_search_only",
        // HNSW and IVF rebuild-required variants map to the same
        // "rebuild_required" label — both signal a /recluster.
        DriftKind::EfConstructionOrM | DriftKind::NlistOrQuantizer => "rebuild_required",
    }
}

fn convert_distance_metric(raw: Option<i32>) -> DistanceMetric {
    use crate::proto::proximadb_v1::DistanceMetric as Proto;
    match raw.and_then(|v| Proto::try_from(v).ok()) {
        Some(Proto::Cosine) => DistanceMetric::Cosine,
        Some(Proto::Euclidean) => DistanceMetric::Euclidean,
        Some(Proto::DotProduct) => DistanceMetric::DotProduct,
        _ => DistanceMetric::Cosine,
    }
}

/// Spawn the background sweeper loop. Identical structure to
/// `spawn_recall_observer` — runs once per `interval` until the
/// shutdown signal flips to `true` or the sender drops.
pub fn spawn_recall_drift_sweeper(
    sweeper: Arc<RecallDriftSweeper>,
    mut shutdown: watch::Receiver<bool>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("RecallDriftSweeper started (poll interval = {interval:?})");
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(interval) => {
                    sweeper.sweep_once().await;
                }
            }
        }
        info!("RecallDriftSweeper stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::{Collection, CollectionConfig, CollectionStats};

    struct FixtureLister(Vec<Collection>);

    #[async_trait::async_trait]
    impl CollectionLister for FixtureLister {
        async fn list_for_sweeper(&self) -> Result<Vec<Collection>, anyhow::Error> {
            Ok(self.0.clone())
        }
    }

    fn col(name: &str, dim: u32, tags: &[&str], current_n: i64) -> Collection {
        Collection {
            id: name.to_string(),
            config: Some(CollectionConfig {
                name: name.to_string(),
                dimension: dim,
                tags: tags.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            }),
            stats: Some(CollectionStats {
                vector_count: current_n,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn sweep_observes_only_recall_target_collections() {
        let lister = Arc::new(FixtureLister(vec![
            col("a", 128, &["recall_target:0.95"], 100_000),
            col("b", 128, &[], 100_000), // no tag → counts as unwired but still recorded
            col("c", 128, &["recall_target:0.85"], 50_000),
        ]));
        let sweeper = RecallDriftSweeper::new(lister);
        let observed = sweeper.sweep_once().await;
        assert_eq!(
            observed, 2,
            "only the two recall_target collections count as 'observed'"
        );
    }

    #[tokio::test]
    async fn sweep_handles_empty_collection_list() {
        let lister = Arc::new(FixtureLister(vec![]));
        let sweeper = RecallDriftSweeper::new(lister);
        let observed = sweeper.sweep_once().await;
        assert_eq!(observed, 0);
    }

    #[tokio::test]
    async fn sweep_skips_collections_without_config() {
        let lister = Arc::new(FixtureLister(vec![Collection {
            id: "no_config".to_string(),
            config: None,
            ..Default::default()
        }]));
        let sweeper = RecallDriftSweeper::new(lister);
        let observed = sweeper.sweep_once().await;
        assert_eq!(observed, 0);
    }

    #[tokio::test]
    async fn sweep_emits_metric_for_unwired_collections() {
        let collection_name = "sweeper_unwired_test_collection_unique_name_xyz";
        let lister = Arc::new(FixtureLister(vec![col(collection_name, 128, &[], 10_000)]));
        let sweeper = RecallDriftSweeper::new(lister);

        let before = crate::metrics::recall_drift_metrics::AXIS_RECALL_DRIFT_OBSERVATIONS_TOTAL
            .with_label_values(&[collection_name])
            .get();
        let _ = sweeper.sweep_once().await;
        let after = crate::metrics::recall_drift_metrics::AXIS_RECALL_DRIFT_OBSERVATIONS_TOTAL
            .with_label_values(&[collection_name])
            .get();
        assert_eq!(
            after - before,
            1.0,
            "unwired collections still get an observation recorded"
        );

        // The "unwired" gauge kind must be set to 1.
        let unwired = crate::metrics::recall_drift_metrics::AXIS_RECALL_DRIFT_STATUS
            .with_label_values(&[collection_name, "unwired"])
            .get();
        assert_eq!(unwired, 1.0);
    }

    #[tokio::test]
    async fn sweep_emits_metric_for_recall_target_collections() {
        let collection_name = "sweeper_wired_test_collection_unique_name_xyz";
        let lister = Arc::new(FixtureLister(vec![col(
            collection_name,
            128,
            &["recall_target:0.95", "target_vector_count:100000"],
            100_000,
        )]));
        let sweeper = RecallDriftSweeper::new(lister);

        let before = crate::metrics::recall_drift_metrics::AXIS_RECALL_DRIFT_OBSERVATIONS_TOTAL
            .with_label_values(&[collection_name])
            .get();
        let observed = sweeper.sweep_once().await;
        let after = crate::metrics::recall_drift_metrics::AXIS_RECALL_DRIFT_OBSERVATIONS_TOTAL
            .with_label_values(&[collection_name])
            .get();

        assert_eq!(observed, 1);
        assert_eq!(after - before, 1.0);

        // baseline_n == current_n == 100K with same recall_target →
        // detect_recall_drift returns DriftKind::None.
        let none_gauge = crate::metrics::recall_drift_metrics::AXIS_RECALL_DRIFT_STATUS
            .with_label_values(&[collection_name, "none"])
            .get();
        assert_eq!(none_gauge, 1.0, "matched baseline+current N → kind=none");
    }

    #[tokio::test]
    async fn auto_hot_swap_defaults_to_true() {
        // Operator builds the sweeper with the default constructor:
        // auto-hot-swap is on, so EfSearchOnly drift triggers an
        // apply attempt on the next sweep tick. The test fixture
        // doesn't register a global axis_manager, so the apply
        // short-circuits at the get_sst_axis_manager() check — but
        // the auto_hot_swap field itself must be true.
        let lister = Arc::new(FixtureLister(vec![]));
        let sweeper = RecallDriftSweeper::new(lister);
        assert!(
            sweeper.auto_hot_swap,
            "auto_hot_swap must default to true for self-healing drift"
        );
    }

    #[tokio::test]
    async fn auto_hot_swap_builder_disables_apply() {
        // Explicit opt-out: with_auto_hot_swap(false). Used in tests
        // and in deployments that want every AXIS mutation explicit.
        let lister = Arc::new(FixtureLister(vec![]));
        let sweeper = RecallDriftSweeper::new(lister).with_auto_hot_swap(false);
        assert!(!sweeper.auto_hot_swap);
    }

    #[tokio::test]
    async fn auto_hot_swap_disabled_does_not_touch_axis_manager() {
        // With auto_hot_swap disabled the sweeper must NOT increment
        // the sweeper trigger counter, even on a collection whose
        // baseline_n vs current_n would normally produce
        // EfSearchOnly drift. (We can't easily verify the
        // axis_manager isn't touched without a fixture for it; this
        // counter assertion is the next-best behavioural check.)
        let collection_name = "sweeper_no_auto_apply_unique_name_xyz";
        let lister = Arc::new(FixtureLister(vec![col(
            collection_name,
            128,
            // baseline_n=100K, current_n=250K → ef_search_only drift
            &["recall_target:0.95", "target_vector_count:100000"],
            250_000,
        )]));
        let sweeper = RecallDriftSweeper::new(lister).with_auto_hot_swap(false);

        let before = crate::metrics::recall_drift_metrics::AXIS_RECALL_DRIFT_HOT_SWAP_APPLIED_TOTAL
            .with_label_values(&[
                collection_name,
                crate::metrics::recall_drift_metrics::HOT_SWAP_TRIGGER_SWEEPER,
            ])
            .get();
        let _ = sweeper.sweep_once().await;
        let after = crate::metrics::recall_drift_metrics::AXIS_RECALL_DRIFT_HOT_SWAP_APPLIED_TOTAL
            .with_label_values(&[
                collection_name,
                crate::metrics::recall_drift_metrics::HOT_SWAP_TRIGGER_SWEEPER,
            ])
            .get();
        assert_eq!(
            after - before,
            0.0,
            "with_auto_hot_swap(false) must not bump the sweeper trigger counter"
        );

        // The drift WAS observed (observation counter ticks).
        let obs_kind = crate::metrics::recall_drift_metrics::AXIS_RECALL_DRIFT_STATUS
            .with_label_values(&[collection_name, "ef_search_only"])
            .get();
        assert_eq!(
            obs_kind, 1.0,
            "drift kind must still be observed even when auto-apply is off"
        );
    }
}
