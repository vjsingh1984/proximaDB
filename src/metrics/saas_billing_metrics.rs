// Copyright 2026 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Prometheus metrics for SaaS MVP Billing and Consumption Telemetry.
//!
//! Exposes exact request/byte-level telemetry required for consumption-based pricing.
//! Metrics are attributed by `tenant_id` to enforce the SaaS multi-tenancy mandates.

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
