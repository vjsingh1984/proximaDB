// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! TD-RDSTRAT-8 two-level IVF coarse-probe operator metrics (SST engine).
//!
//! Aggregate operator view of the coarse probe — whether it is engaging, how
//! much of Region-A it avoids reading, and how often it falls back to the whole
//! region scan. These are the signals the default-on promotion gate watches
//! (`proximadb_ivf_whole_region_fallback_total` climbing ⇒ nprobe too small).
//!
//! Global counters (no collection label): the per-query, per-tenant *durable*
//! record lives in the io_trace warehouse `VectorAnnPayload` (ADR-066); this
//! Prometheus surface is the real-time aggregate operator dashboard. A
//! collection label can be layered on once the warehouse data shows
//! per-collection demand — keeping cardinality intentional, not eager.

use lazy_static::lazy_static;
use prometheus::{IntCounter, register_int_counter};

fn counter(name: &str, help: &str) -> IntCounter {
    register_int_counter!(name, help).unwrap_or_else(|_| {
        IntCounter::new(format!("{name}_fallback"), help)
            .unwrap_or_else(|_| unreachable!("valid counter metric descriptor"))
    })
}

// NOTE: unlabeled AGGREGATE operator surface. The per-tenant billing versions
// in metrics/consumption_metrics.rs own the canonical
// `proximadb_ivf_*_total{tenant_id}` names (sum over tenant = this aggregate);
// these carry `_agg_` to avoid a duplicate-registration crash when both init
// on a probe-armed search (default-ON since #1210). In-process readers use the
// Rust statics below, unaffected by the prometheus name.
lazy_static! {
    pub static ref IVF_CELLS_TOTAL: IntCounter = counter(
        "proximadb_ivf_cells_agg_total",
        "Persisted coarse centroids seen by the TD-RDSTRAT-8 two-level IVF coarse probe",
    );
    pub static ref IVF_CELLS_PROBED: IntCounter = counter(
        "proximadb_ivf_cells_probed_agg_total",
        "Coarse centroids ranked in RAM by the nprobe-scoped TD-RDSTRAT-8 probe",
    );
    pub static ref IVF_PROBED_ROWS: IntCounter = counter(
        "proximadb_ivf_probed_rows_agg_total",
        "Rows ranged-read from the probed cells' Region-A extents (TD-RDSTRAT-8)",
    );
    pub static ref IVF_FETCH_ROUNDS: IntCounter = counter(
        "proximadb_ivf_fetch_rounds_agg_total",
        "Coalesced Region-A ranged-read runs issued by the TD-RDSTRAT-8 probe (the GET round-trip budget it pays)",
    );
    pub static ref IVF_WHOLE_REGION_FALLBACK: IntCounter = counter(
        "proximadb_ivf_whole_region_fallback_total",
        "Segments where the armed TD-RDSTRAT-8 coarse probe missed and the read fell back to the whole Region-A scan",
    );
}

/// Record a coarse-probe outcome to the aggregate operator surface. Mirrors the
/// durable per-query record in `io_trace::record_ivf_coarse_probe` (which lands
/// in the warehouse `VectorAnnPayload`); this is the real-time operator aggregate.
pub fn record_ivf_coarse_probe(
    cells_total: u64,
    cells_probed: u64,
    probed_rows: u64,
    fetch_rounds: u64,
    whole_region_fallback: bool,
) {
    IVF_CELLS_TOTAL.inc_by(cells_total);
    IVF_CELLS_PROBED.inc_by(cells_probed);
    IVF_PROBED_ROWS.inc_by(probed_rows);
    IVF_FETCH_ROUNDS.inc_by(fetch_rounds);
    if whole_region_fallback {
        IVF_WHOLE_REGION_FALLBACK.inc();
    }
}
