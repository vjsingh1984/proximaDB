/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Cross-modal fusion metrics (T1.1).
//!
//! Tracks fusion search operations across modalities (vector, graph, document,
//! relational) for observability and cost model calibration.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Global fusion metrics registry.
///
/// Thread-safe singleton for tracking fusion operations across all modalities.
/// Metrics are emitted to Prometheus for observability.
pub struct FusionMetrics {
    /// Total number of fusion operations performed.
    total_fusions: AtomicU64,

    /// Total number of source modalities fused (vector + graph + document + ...).
    total_sources_fused: AtomicU64,

    /// Total number of source modalities skipped (unavailable, timeout, or empty).
    total_sources_skipped: AtomicU64,

    /// Total number of candidates input to fusion (pre-calibration).
    total_candidates_in: AtomicU64,

    /// Total number of items output from fusion (post-calibration/reranking).
    total_items_out: AtomicU64,

    /// Total fusion latency in microseconds.
    total_latency_us: AtomicU64,

    /// Number of latency samples (for computing average).
    latency_samples: AtomicU64,
}

impl FusionMetrics {
    /// Create a new fusion metrics instance.
    pub fn new() -> Self {
        Self {
            total_fusions: AtomicU64::new(0),
            total_sources_fused: AtomicU64::new(0),
            total_sources_skipped: AtomicU64::new(0),
            total_candidates_in: AtomicU64::new(0),
            total_items_out: AtomicU64::new(0),
            total_latency_us: AtomicU64::new(0),
            latency_samples: AtomicU64::new(0),
        }
    }

    /// Record a fusion operation.
    ///
    /// Called from the fusion service after each fusion search completes.
    pub fn record_fusion(
        &self,
        sources_fused: usize,
        sources_skipped: usize,
        candidates_in: usize,
        items_out: usize,
        latency: Duration,
    ) {
        self.total_fusions.fetch_add(1, Ordering::Relaxed);
        self.total_sources_fused
            .fetch_add(sources_fused as u64, Ordering::Relaxed);
        self.total_sources_skipped
            .fetch_add(sources_skipped as u64, Ordering::Relaxed);
        self.total_candidates_in
            .fetch_add(candidates_in as u64, Ordering::Relaxed);
        self.total_items_out
            .fetch_add(items_out as u64, Ordering::Relaxed);
        self.total_latency_us
            .fetch_add(latency.as_micros() as u64, Ordering::Relaxed);
        self.latency_samples.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the current metrics snapshot.
    pub fn snapshot(&self) -> FusionMetricsSnapshot {
        let samples = self.latency_samples.load(Ordering::Relaxed);
        FusionMetricsSnapshot {
            total_fusions: self.total_fusions.load(Ordering::Relaxed),
            total_sources_fused: self.total_sources_fused.load(Ordering::Relaxed),
            total_sources_skipped: self.total_sources_skipped.load(Ordering::Relaxed),
            total_candidates_in: self.total_candidates_in.load(Ordering::Relaxed),
            total_items_out: self.total_items_out.load(Ordering::Relaxed),
            avg_latency_us: self
                .total_latency_us
                .load(Ordering::Relaxed)
                .checked_div(samples)
                .unwrap_or(0),
        }
    }
}

impl Default for FusionMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of fusion metrics for export.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FusionMetricsSnapshot {
    /// Total number of fusion operations.
    pub total_fusions: u64,

    /// Total number of sources fused across all operations.
    pub total_sources_fused: u64,

    /// Total number of sources skipped across all operations.
    pub total_sources_skipped: u64,

    /// Total candidates input across all operations.
    pub total_candidates_in: u64,

    /// Total items output across all operations.
    pub total_items_out: u64,

    /// Average fusion latency in microseconds.
    pub avg_latency_us: u64,
}

/// Global fusion metrics registry.
static GLOBAL_FUSION_METRICS: std::sync::OnceLock<Arc<FusionMetrics>> = std::sync::OnceLock::new();

/// Get or create the global fusion metrics registry.
pub fn global_fusion_metrics() -> Option<Arc<FusionMetrics>> {
    GLOBAL_FUSION_METRICS.get().cloned()
}

/// Initialize the global fusion metrics registry.
pub fn init_global_fusion_metrics() {
    GLOBAL_FUSION_METRICS.get_or_init(|| Arc::new(FusionMetrics::new()));
}

/// Record a fusion operation to the global registry.
///
/// Convenience function for recording metrics without explicitly
/// accessing the global registry.
pub fn record_fusion(
    sources_fused: usize,
    sources_skipped: usize,
    candidates_in: usize,
    items_out: usize,
    latency: Duration,
) {
    if let Some(metrics) = global_fusion_metrics() {
        metrics.record_fusion(
            sources_fused,
            sources_skipped,
            candidates_in,
            items_out,
            latency,
        );
    }
}
