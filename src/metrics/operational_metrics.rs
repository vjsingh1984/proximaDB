// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-METRICS-1: real operational counters, registered in the process-default
//! prometheus registry (scraped by `/metrics/prometheus` via
//! `prometheus::gather()`).
//!
//! These replace the never-incremented struct-exporter stanzas
//! (`QueryMetrics`/`StorageMetrics`) that used to render
//! `proximadb_queries_total 0` — the names are reclaimed here as live
//! counters (the stale hand-written stanzas were removed from
//! `export_system_metrics` in the same change, so there is no name
//! collision on the scrape output).
//!
//! Covered signals:
//! - **query rate/outcome**: `proximadb_queries_total` /
//!   `proximadb_queries_failed_total` + `proximadb_search_latency_seconds`,
//!   incremented at the query-facade search entry.
//! - **cache effectiveness** (the co-design hot-path signal — DRAM vs object
//!   store): survivor-range cache hits/misses/bytes (mirrored from
//!   `TenantCache`'s internal atomics) and segment-invariants cache
//!   hits/misses/bytes (counted at the lookup boundary).

use lazy_static::lazy_static;
use prometheus::{
    Histogram, IntCounter, IntGauge, register_histogram, register_int_counter, register_int_gauge,
};

fn counter(name: &str, help: &str) -> IntCounter {
    register_int_counter!(name, help).unwrap_or_else(|_| {
        IntCounter::new(format!("{name}_fallback"), help)
            .unwrap_or_else(|_| unreachable!("valid counter metric descriptor"))
    })
}

fn gauge(name: &str, help: &str) -> IntGauge {
    register_int_gauge!(name, help).unwrap_or_else(|_| {
        IntGauge::new(format!("{name}_fallback"), help)
            .unwrap_or_else(|_| unreachable!("valid gauge metric descriptor"))
    })
}

fn histogram(name: &str, help: &str, buckets: Vec<f64>) -> Histogram {
    register_histogram!(name, help, buckets.clone()).unwrap_or_else(|_| {
        Histogram::with_opts(
            prometheus::HistogramOpts::new(format!("{name}_fallback"), help).buckets(buckets),
        )
        .unwrap_or_else(|_| unreachable!("valid histogram metric descriptor"))
    })
}

lazy_static! {
    /// Vector/record searches served through the query facade (all surfaces).
    pub static ref QUERIES_TOTAL: IntCounter = counter(
        "proximadb_queries_total",
        "Total search queries processed through the query facade",
    );
    pub static ref QUERIES_FAILED_TOTAL: IntCounter = counter(
        "proximadb_queries_failed_total",
        "Search queries that returned an error from the query facade",
    );
    /// Buckets span DRAM-hot (sub-ms) through cold object-store scans (10s).
    pub static ref SEARCH_LATENCY_SECONDS: Histogram = histogram(
        "proximadb_search_latency_seconds",
        "End-to-end search latency at the query facade",
        vec![0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
    );

    /// Survivor-range cache (SST engine RAM cache over survivor-block ranges).
    /// Absolute values mirrored from `TenantCache`'s internal atomics after
    /// each lookup — gauges, not counters, because the source is a running
    /// total we sample rather than an event we observe.
    pub static ref SURVIVOR_CACHE_HITS: IntGauge = gauge(
        "proximadb_survivor_cache_hits",
        "Cumulative survivor-range cache hits (0 GETs served from DRAM)",
    );
    pub static ref SURVIVOR_CACHE_MISSES: IntGauge = gauge(
        "proximadb_survivor_cache_misses",
        "Cumulative survivor-range cache misses (paid object-store GETs)",
    );
    pub static ref SURVIVOR_CACHE_BYTES: IntGauge = gauge(
        "proximadb_survivor_cache_bytes",
        "Resident bytes in the survivor-range cache",
    );

    /// Segment-invariants cache (footer/region-A metadata; a hit skips 3
    /// ranged GETs per segment per query).
    pub static ref SEGMENT_INVARIANTS_CACHE_HITS_TOTAL: IntCounter = counter(
        "proximadb_segment_invariants_cache_hits_total",
        "Segment-invariants cache hits (footer + A0 metadata served from DRAM)",
    );
    pub static ref SEGMENT_INVARIANTS_CACHE_MISSES_TOTAL: IntCounter = counter(
        "proximadb_segment_invariants_cache_misses_total",
        "Segment-invariants cache misses (footer + A0 metadata fetched from storage)",
    );
    pub static ref SEGMENT_INVARIANTS_CACHE_BYTES: IntGauge = gauge(
        "proximadb_segment_invariants_cache_bytes",
        "Resident bytes in the segment-invariants cache",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_metrics_register_and_move() {
        let q0 = QUERIES_TOTAL.get();
        QUERIES_TOTAL.inc();
        assert_eq!(QUERIES_TOTAL.get(), q0 + 1);
        SEARCH_LATENCY_SECONDS.observe(0.02);
        SURVIVOR_CACHE_HITS.set(7);
        assert_eq!(SURVIVOR_CACHE_HITS.get(), 7);
        SEGMENT_INVARIANTS_CACHE_HITS_TOTAL.inc();
        assert!(SEGMENT_INVARIANTS_CACHE_HITS_TOTAL.get() >= 1);
    }
}
