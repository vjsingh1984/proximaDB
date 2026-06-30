#![allow(clippy::unwrap_used, clippy::expect_used)]
// dedicated metric-registration module: static registration is infallible / fail-fast at startup;
// lazy_static can't carry per-site allows.
// Copyright 2026 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Read-route decision observability (co-design C4 operator surface).
//!
//! The `ComputeScheduler` route decision was previously observable only via a
//! `tracing::debug!` line — invisible in production dashboards. These counters
//! and the consult-latency histogram make the route distribution, the
//! static/override/explore split, and the cost of the (now lock-free) consult
//! itself operationally visible. Labels are deliberately low-cardinality:
//! `backend` is the canonical engine label, `shape_class` the coarse cost-model
//! class key, and `source` one of `static` / `override_exploit` / `override_explore`.
//!
//! `ROUTE_OLAP_ON_NATIVE_TOTAL` is an operator nudge: an analytic (`olap/*`)
//! query served by the Native/Volcano row engine is paying N row-gets instead of
//! a columnar scan — the table is a materialization candidate.

use std::time::Duration;

use lazy_static::lazy_static;
use prometheus::{
    CounterVec, HistogramOpts, HistogramVec, Opts, register_counter_vec, register_histogram_vec,
};
use tracing::error;

fn register_counter_vec_safe(name: &str, help: &str, labels: &[&str]) -> CounterVec {
    match register_counter_vec!(name, help, labels) {
        Ok(metric) => metric,
        Err(reg_err) => {
            error!("Failed to register {}: {}", name, reg_err);
            CounterVec::new(Opts::new(name, help), labels).unwrap_or_else(|_| {
                CounterVec::new(Opts::new(format!("{}_fallback", name), help), labels)
                    .unwrap_or_else(|_| unreachable!("valid counter metric descriptor"))
            })
        }
    }
}

fn register_histogram_vec_safe(
    name: &str,
    help: &str,
    labels: &[&str],
    buckets: Vec<f64>,
) -> HistogramVec {
    match register_histogram_vec!(name, help, labels, buckets.clone()) {
        Ok(metric) => metric,
        Err(reg_err) => {
            error!("Failed to register {}: {}", name, reg_err);
            HistogramVec::new(
                HistogramOpts::new(name, help).buckets(buckets.clone()),
                labels,
            )
            .unwrap_or_else(|_| {
                HistogramVec::new(
                    HistogramOpts::new(format!("{}_fallback", name), help).buckets(buckets),
                    labels,
                )
                .unwrap_or_else(|_| unreachable!("valid histogram metric descriptor"))
            })
        }
    }
}

lazy_static! {
    /// Route decisions by backend, shape-class, and decision source.
    pub static ref ROUTE_DECISIONS_TOTAL: CounterVec = register_counter_vec_safe(
        "proximadb_route_decisions_total",
        "Read-route decisions by backend, shape-class, and source (static | override_exploit | override_explore)",
        &["backend", "shape_class", "source"],
    );

    /// Consult (route-decision) latency. Proves the consult stays sub-microsecond
    /// now that it is a lock-free `ArcSwap` load + `HashMap` get.
    pub static ref ROUTE_CONSULT_DURATION_SECONDS: HistogramVec = register_histogram_vec_safe(
        "proximadb_route_consult_duration_seconds",
        "Read-route decision latency, by whether a cost model was consulted",
        &["had_model"],
        // Sub-microsecond to ~1ms — the consult is a hot-path, lock-free read.
        vec![1e-7, 5e-7, 1e-6, 2e-6, 5e-6, 1e-5, 5e-5, 1e-4, 1e-3],
    );

    /// Analytic (`olap/*`) queries served by the Native/Volcano row engine — a
    /// materialization nudge (the query would be cheaper over a Parquet base).
    pub static ref ROUTE_OLAP_ON_NATIVE_TOTAL: CounterVec = register_counter_vec_safe(
        "proximadb_route_olap_on_native_total",
        "Analytic queries routed to the Native row engine (unmaterialized) — consider ALTER TABLE MATERIALIZE",
        &["shape_class"],
    );
}

/// Record one route-decision outcome. `source` is the [`crate::query::compute_scheduler::RouteSource`]
/// label string (`static` / `override_exploit` / `override_explore`).
pub fn record_decision(backend_label: &str, shape_class: &str, source: &str) {
    ROUTE_DECISIONS_TOTAL
        .with_label_values(&[backend_label, shape_class, source])
        .inc();
}

/// Record an analytic query served by the Native row engine (operator nudge).
pub fn record_olap_on_native(shape_class: &str) {
    ROUTE_OLAP_ON_NATIVE_TOTAL
        .with_label_values(&[shape_class])
        .inc();
}

/// Observe one consult's wall-clock duration. `had_model` distinguishes the
/// pure-static path (no consult) from the advised path.
pub fn observe_consult_duration(had_model: bool, duration: Duration) {
    ROUTE_CONSULT_DURATION_SECONDS
        .with_label_values(&[if had_model { "true" } else { "false" }])
        .observe(duration.as_secs_f64());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_decision_increments_the_right_label_set() {
        // nextest process-isolation: this process's registry starts empty, so an
        // absolute count is meaningful here.
        let before = ROUTE_DECISIONS_TOTAL
            .with_label_values(&["DataFusionLocal", "olap/parquet", "static"])
            .get();
        record_decision("DataFusionLocal", "olap/parquet", "static");
        let after = ROUTE_DECISIONS_TOTAL
            .with_label_values(&["DataFusionLocal", "olap/parquet", "static"])
            .get();
        assert_eq!(after - before, 1.0);
    }

    #[test]
    fn record_olap_on_native_increments() {
        let before = ROUTE_OLAP_ON_NATIVE_TOTAL
            .with_label_values(&["olap/native"])
            .get();
        record_olap_on_native("olap/native");
        record_olap_on_native("olap/native");
        let after = ROUTE_OLAP_ON_NATIVE_TOTAL
            .with_label_values(&["olap/native"])
            .get();
        assert_eq!(after - before, 2.0);
    }

    #[test]
    fn observe_consult_duration_lands_a_sample() {
        // A fresh histogram with one observed sample has count 1.
        let hist = ROUTE_CONSULT_DURATION_SECONDS.with_label_values(&["true"]);
        let before = hist.get_sample_count();
        observe_consult_duration(true, Duration::from_nanos(500));
        let after = hist.get_sample_count();
        assert_eq!(after - before, 1u64);
    }
}
