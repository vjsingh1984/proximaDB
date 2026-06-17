#![allow(clippy::unwrap_used, clippy::expect_used)]
// dedicated metric-registration module: static registration is infallible / fail-fast at startup; lazy_static can't carry per-site allows
// Copyright 2026 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Prometheus consumption telemetry: request/byte-level operational counters
//!
//! Generic per-tenant usage counters (object-store ops, storage byte-seconds,
//! task time). Pricing/billing is an operator/control-plane concern — this OSS
//! module only emits neutral telemetry attributed by `tenant_id`.

use lazy_static::lazy_static;
use prometheus::{CounterVec, GaugeVec, Opts, register_counter_vec, register_gauge_vec};
use tracing::error;

lazy_static! {
    pub static ref OBJECT_STORE_OPS_TOTAL: CounterVec = register_counter_vec!(
        Opts::new(
            "proximadb_object_store_ops_total",
            "Total number of object store I/O operations (put, get, list, delete)"
        ),
        &["tenant_id", "operation"]
    )
    .unwrap_or_else(|err| {
        error!(
            "failed to register proximadb_object_store_ops_total: {}",
            err
        );
        CounterVec::new(
            Opts::new("proximadb_object_store_ops_total", ""),
            &["tenant_id", "operation"],
        )
        .unwrap()
    });
    pub static ref STORAGE_BYTES_SECONDS: GaugeVec = register_gauge_vec!(
        Opts::new(
            "proximadb_storage_bytes_seconds",
            "GB-seconds or raw bytes stored per tenant"
        ),
        &["tenant_id", "storage_type"]
    )
    .unwrap_or_else(|err| {
        error!(
            "failed to register proximadb_storage_bytes_seconds: {}",
            err
        );
        GaugeVec::new(
            Opts::new("proximadb_storage_bytes_seconds", ""),
            &["tenant_id", "storage_type"],
        )
        .unwrap()
    });
    pub static ref TASK_EXECUTION_TIME_MS: CounterVec = register_counter_vec!(
        Opts::new(
            "proximadb_task_execution_time_ms",
            "Total analytical task execution time in milliseconds (DataFusion/Polars compute)"
        ),
        &["tenant_id", "engine"]
    )
    .unwrap_or_else(|err| {
        error!(
            "failed to register proximadb_task_execution_time_ms: {}",
            err
        );
        CounterVec::new(
            Opts::new("proximadb_task_execution_time_ms", ""),
            &["tenant_id", "engine"],
        )
        .unwrap()
    });
    /// Per-tenant cache footprint + hit/miss snapshot (the multitenant cache
    /// observability for fairness/noisy-neighbor + chargeback). Gauges carry the
    /// current cumulative totals per tenant from `TenantCache::tenant_stats`.
    pub static ref CACHE_TENANT_STATS: GaugeVec = register_gauge_vec!(
        Opts::new(
            "proximadb_cache_tenant",
            "Per-tenant cache stats snapshot (metric label selects bytes/hits/misses/hit_ratio/evictions)"
        ),
        &["tenant_id", "cache", "metric"]
    )
    .unwrap_or_else(|err| {
        error!("failed to register proximadb_cache_tenant: {}", err);
        GaugeVec::new(
            Opts::new("proximadb_cache_tenant", ""),
            &["tenant_id", "cache", "metric"],
        )
        .unwrap()
    });
}

/// Helper to record an object store operation.
pub fn record_object_store_op(tenant_id: Option<&str>, operation: &str) {
    let t_id = tenant_id.unwrap_or("default");
    OBJECT_STORE_OPS_TOTAL
        .with_label_values(&[t_id, operation])
        .inc();
}

/// Helper to update storage capacity usage gauge.
pub fn record_storage_bytes(tenant_id: Option<&str>, storage_type: &str, bytes: f64) {
    let t_id = tenant_id.unwrap_or("default");
    STORAGE_BYTES_SECONDS
        .with_label_values(&[t_id, storage_type])
        .set(bytes);
}

/// Helper to record compute execution time.
pub fn record_task_execution_time(tenant_id: Option<&str>, engine: &str, duration_ms: f64) {
    let t_id = tenant_id.unwrap_or("default");
    TASK_EXECUTION_TIME_MS
        .with_label_values(&[t_id, engine])
        .inc_by(duration_ms);
}

/// Publish a per-tenant cache stats snapshot (e.g. from the footer cache's
/// `tenant_stats()`) onto the `proximadb_cache_tenant` gauge. `cache` names the
/// cache (e.g. "footer").
pub fn record_cache_tenant_stats(cache: &str, stats: &[proximadb_cache::TenantCacheStat]) {
    for s in stats {
        let g = |metric: &str, v: f64| {
            CACHE_TENANT_STATS
                .with_label_values(&[s.tenant.as_str(), cache, metric])
                .set(v);
        };
        g("bytes", s.bytes as f64);
        g("hits", s.hits as f64);
        g("misses", s.misses as f64);
        g("evictions", s.evictions as f64);
        g("hit_ratio", s.hit_ratio);
    }
}
