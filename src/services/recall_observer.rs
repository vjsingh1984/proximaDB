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

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use crate::index::AxisManager;
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
    sample_size: usize,
    k: usize,
    recall_floor: f32,
}

impl RecallObserver {
    pub fn new(axis: Arc<AxisManager>, vector_ops: Arc<VectorOperationsService>) -> Self {
        Self {
            axis,
            vector_ops,
            sample_size: DEFAULT_SAMPLE_SIZE,
            k: DEFAULT_RECALL_K,
            recall_floor: DEFAULT_RECALL_FLOOR,
        }
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
            }
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
