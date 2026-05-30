//! Phase-5 recall observer — makes the TD-075 quantized gate self-tuning.
//!
//! A background task that periodically measures quantized-vs-exact recall per
//! collection and feeds the outcome into the shared `RecallProbeGate`
//! (`AxisManager::probe_and_observe`). After `passes_required` consecutive
//! passes the gate opens and `AxisManager::query_ivf` starts selecting the
//! quantized accelerator; a single FAIL closes it again. Probe queries are
//! sampled record embeddings (the IVF index is query-only), pulled via the v2
//! storage-inclusive scan.
//!
//! Scope (this slice): a constant recall floor + sample size + interval. Reading
//! the per-collection `RecallSlo` from the catalog precision policy, config
//! knobs, tenant-scoped probes, and Prometheus metrics are follow-ups.
//!
//! F1 trigger arm (2026-05-30): besides opening/closing the quantized gate, a
//! recall **regression** (the gate transitioning open -> closed: recall was
//! acceptable, then dropped below the floor) is a *quality*-driven discovery
//! signal orthogonal to write volume — the DriftWatcher only sees write
//! activity, but recall can degrade from distribution shift with no new writes.
//! On that transition the observer emits `TriggerSignal::RecallDegraded`, which
//! the trigger maps to a (coalesced, non-destructive) Recluster. Firing only on
//! the transition is inherently rate-limited: it signals once per regression
//! episode and won't re-fire until recall recovers (3 passes reopen the gate)
//! and drops again — no thrash even if a collection stays below the floor.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use crate::index::AxisManager;
use crate::services::discovery::{DiscoveryService, TriggerSignal};
use crate::services::VectorOperationsService;

/// Default interval between observation passes.
pub const DEFAULT_OBSERVE_INTERVAL: Duration = Duration::from_secs(300);
/// Probe queries sampled per collection per pass.
const DEFAULT_SAMPLE_SIZE: usize = 64;
/// recall@k k.
const DEFAULT_RECALL_K: usize = 10;
/// Mean recall at/above which a probe passes (quantized recall is acceptable).
const DEFAULT_RECALL_FLOOR: f32 = 0.95;

/// Drives recall probes over collections with a quantized IVF index.
pub struct RecallObserver {
    axis: Arc<AxisManager>,
    vector_ops: Arc<VectorOperationsService>,
    /// When set, a recall regression (gate open -> closed) emits a discovery
    /// `RecallDegraded` signal. `None` => probe-only (gate tuning, no trigger).
    discovery: Option<Arc<DiscoveryService>>,
    /// Last observed gate state per collection (internal id), so the observer
    /// can detect the open -> closed *transition* rather than re-firing on every
    /// sustained-closed pass.
    prev_gate_open: Mutex<HashMap<String, bool>>,
    sample_size: usize,
    k: usize,
    recall_floor: f32,
}

impl RecallObserver {
    pub fn new(axis: Arc<AxisManager>, vector_ops: Arc<VectorOperationsService>) -> Self {
        Self {
            axis,
            vector_ops,
            discovery: None,
            prev_gate_open: Mutex::new(HashMap::new()),
            sample_size: DEFAULT_SAMPLE_SIZE,
            k: DEFAULT_RECALL_K,
            recall_floor: DEFAULT_RECALL_FLOOR,
        }
    }

    /// Attach the discovery service so recall regressions trigger a recluster.
    pub fn with_discovery(mut self, discovery: Arc<DiscoveryService>) -> Self {
        self.discovery = Some(discovery);
        self
    }

    /// One observation pass over every collection that has a quantized IVF index.
    pub async fn run_once(&self) {
        let collections = self.axis.quantized_ivf_collections().await;
        for collection_id in collections {
            let records = match self
                .vector_ops
                .list_all_records_with_tenant_context(&collection_id, None)
                .await
            {
                Ok(records) => records,
                Err(err) => {
                    warn!("recall observer: scan of '{collection_id}' failed: {err}");
                    continue;
                }
            };

            // Sample record embeddings as probe queries (the IVF index is
            // query-only and exposes no stored vectors).
            let queries: Vec<Vec<f32>> = records
                .iter()
                .take(self.sample_size)
                .filter_map(|r| r.embeddings.first().map(|c| c.as_fp32_cow().into_owned()))
                .filter(|v| !v.is_empty())
                .collect();
            if queries.is_empty() {
                continue;
            }

            if let Some(state) = self
                .axis
                .probe_and_observe(&collection_id, &queries, self.k, self.recall_floor)
                .await
            {
                info!(
                    "recall observer: '{collection_id}' gate_open={} passes={}",
                    state.gate_open, state.consecutive_passes
                );
                // Detect the open -> closed transition against the prior pass.
                let prev = self
                    .prev_gate_open
                    .lock()
                    .expect("prev_gate_open poisoned")
                    .insert(collection_id.clone(), state.gate_open);
                if Self::recall_regressed(prev, state.gate_open) {
                    self.signal_recall_degraded(&collection_id).await;
                }
            }
        }
    }

    /// A recall regression worth a recluster: the gate was open last pass and is
    /// closed now. A first-ever-closed (`prev == None`) or sustained-closed
    /// (`prev == Some(false)`) state is not a fresh regression, so it never fires.
    fn recall_regressed(prev_open: Option<bool>, new_open: bool) -> bool {
        matches!(prev_open, Some(true)) && !new_open
    }

    /// Emit a `RecallDegraded` discovery signal for the collection. Resolves the
    /// AxisManager index key (internal id) to the user-facing name so the job
    /// coalesces with — and shares a recluster baseline with — DriftWatcher jobs
    /// (the discovery pipeline keys by name). No-op when discovery isn't wired.
    async fn signal_recall_degraded(&self, internal_id: &str) {
        let Some(discovery) = self.discovery.as_ref() else {
            return;
        };
        let name = self
            .vector_ops
            .resolve_collection_name(internal_id)
            .await
            .unwrap_or_else(|| internal_id.to_string());
        if discovery
            .on_signal(&name, TriggerSignal::RecallDegraded)
            .is_some()
        {
            info!("recall observer: recall regression on '{name}' -> recluster signal");
        }
    }
}

/// Spawn the background observer loop. Probes once per `interval`, until the
/// shutdown signal flips to `true` (or the sender is dropped). Mirrors
/// `spawn_discovery_executor`.
pub fn spawn_recall_observer(
    observer: Arc<RecallObserver>,
    mut shutdown: watch::Receiver<bool>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("RecallObserver started (poll interval = {interval:?})");
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(interval) => {
                    observer.run_once().await;
                }
            }
        }
        info!("RecallObserver stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::RecallObserver;

    #[test]
    fn recall_regression_fires_only_on_open_to_closed() {
        // Gate was open last pass and is closed now: a fresh recall regression
        // — the one case worth a recluster.
        assert!(RecallObserver::recall_regressed(Some(true), false));

        // Still open: no regression.
        assert!(!RecallObserver::recall_regressed(Some(true), true));
        // Sustained closed: not a *fresh* regression — must not re-fire every
        // pass while recall stays low (avoids recluster thrash).
        assert!(!RecallObserver::recall_regressed(Some(false), false));
        // Recovery (closed -> open): not a regression.
        assert!(!RecallObserver::recall_regressed(Some(false), true));
        // First observation ever, closed: never seen healthy, so a drop isn't
        // indicated — don't fire.
        assert!(!RecallObserver::recall_regressed(None, false));
        // First observation ever, open: obviously not a regression.
        assert!(!RecallObserver::recall_regressed(None, true));
    }
}
