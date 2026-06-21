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

use std::sync::LazyLock;

use lazy_static::lazy_static;
use prometheus::{CounterVec, GaugeVec, Opts, register_counter_vec, register_gauge_vec};
use tracing::error;

// Register-or-fallback helpers. Static registration is infallible / fail-fast at
// startup (see module-level allow); centralizing the fallback keeps the single
// sanctioned panic site per vec type instead of repeating it at every metric.
fn registered_counter_vec(name: &'static str, help: &'static str, labels: &[&str]) -> CounterVec {
    register_counter_vec!(Opts::new(name, help), labels).unwrap_or_else(|err| {
        error!("failed to register {}: {}", name, err);
        CounterVec::new(Opts::new(name, ""), labels).unwrap()
    })
}

fn registered_gauge_vec(name: &'static str, help: &'static str, labels: &[&str]) -> GaugeVec {
    register_gauge_vec!(Opts::new(name, help), labels).unwrap_or_else(|err| {
        error!("failed to register {}: {}", name, err);
        GaugeVec::new(Opts::new(name, ""), labels).unwrap()
    })
}

lazy_static! {
    pub static ref OBJECT_STORE_OPS_TOTAL: CounterVec = registered_counter_vec(
        "proximadb_object_store_ops_total",
        "Total number of object store I/O operations (put, get, list, delete)",
        &["tenant_id", "operation"]
    );
    pub static ref STORAGE_BYTES_SECONDS: GaugeVec = registered_gauge_vec(
        "proximadb_storage_bytes_seconds",
        "GB-seconds or raw bytes stored per tenant",
        &["tenant_id", "storage_type"]
    );
    pub static ref TASK_EXECUTION_TIME_MS: CounterVec = registered_counter_vec(
        "proximadb_task_execution_time_ms",
        "Total analytical task execution time in milliseconds (DataFusion/Polars compute)",
        &["tenant_id", "engine"]
    );
    /// Per-tenant cache footprint + hit/miss snapshot (the multitenant cache
    /// observability for fairness/noisy-neighbor + chargeback). Gauges carry the
    /// current cumulative totals per tenant from `TenantCache::tenant_stats`.
    pub static ref CACHE_TENANT_STATS: GaugeVec = registered_gauge_vec(
        "proximadb_cache_tenant",
        "Per-tenant cache stats snapshot (metric label selects bytes/hits/misses/hit_ratio/evictions)",
        &["tenant_id", "cache", "metric"]
    );
    /// Per-tenant bytes moved OUT of object storage, labelled by AZ locality —
    /// the KEU egress meter (co-design Dimension 2 / §2.2), the one previously
    /// unmetered dimension. Networking (cross-AZ $0.02/GB RT, internet egress
    /// $0.09/GB) is frequently the dominant TCO term, yet same-AZ is free — so
    /// the `az_locality` label is the discriminator that makes egress a billable,
    /// optimizable dimension. Neutral telemetry only: the AZ topology and the $
    /// weights are control-plane (anvaiops) policy.
    pub static ref EGRESS_BYTES_TOTAL: CounterVec = registered_counter_vec(
        "proximadb_egress_bytes_total",
        "Bytes moved out of object storage per tenant, labelled by AZ locality (KEU egress)",
        &["tenant_id", "az_locality"]
    );
}

/// AZ-locality classification of bytes leaving object storage — the KEU egress
/// dimension (§2.2). This is OSS **mechanism**: it records *which locality bucket*
/// the bytes moved through; the real AZ topology and the per-locality dollar
/// weights are commercial-control-plane (anvaiops) **policy**. Same-AZ is the
/// co-designed free path; cross-AZ and internet egress are the chargeable terms
/// the cost model's egress weight consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AzLocality {
    /// Compute, cache, and object store co-located in one AZ over private IP —
    /// the free path. The default, so egress stays inert until a deployment
    /// declares otherwise.
    #[default]
    SameAz,
    /// Bytes crossed an availability-zone boundary (~$0.02/GB round-trip).
    CrossAz,
    /// Bytes left the cloud to the public internet (egress, ~$0.09/GB).
    Internet,
}

impl AzLocality {
    /// Stable metric-label string.
    pub fn as_str(self) -> &'static str {
        match self {
            AzLocality::SameAz => "same_az",
            AzLocality::CrossAz => "cross_az",
            AzLocality::Internet => "internet",
        }
    }

    /// Whether bytes through this locality are chargeable egress (i.e. *not* the
    /// free same-AZ path). Only these bytes feed the per-query trace's
    /// `bytes_cross_az` term, so same-AZ traffic never inflates the egress cost.
    pub fn is_chargeable_egress(self) -> bool {
        !matches!(self, AzLocality::SameAz)
    }

    /// Parse the deployment-declared object-store locality from a string
    /// (`same_az` | `cross_az` | `internet`), accepting a couple of obvious
    /// aliases. Unknown values fall back to the free `SameAz` default.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "cross_az" | "cross-az" | "crossaz" => AzLocality::CrossAz,
            "internet" | "egress" | "public" => AzLocality::Internet,
            _ => AzLocality::SameAz,
        }
    }
}

/// The deployment-declared locality of the object store relative to the compute
/// reading it, from `PROXIMADB_OBJECT_STORE_LOCALITY` (read once). Default
/// `SameAz` — the co-designed free path — so the egress cost term is inert until
/// an operator declares a cross-AZ/internet topology. The actual $ weight per
/// locality remains anvaiops policy; this only selects the neutral bucket.
static OBJECT_STORE_LOCALITY: LazyLock<AzLocality> = LazyLock::new(|| {
    std::env::var("PROXIMADB_OBJECT_STORE_LOCALITY")
        .map(|v| AzLocality::parse(&v))
        .unwrap_or_default()
});

/// The configured object-store egress locality for this deployment.
pub fn object_store_egress_locality() -> AzLocality {
    *OBJECT_STORE_LOCALITY
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

/// Record bytes moved out of object storage for one tenant under `locality` —
/// the single convergent KEU egress meter (Dimension 2). It does two things at
/// once so every I/O boundary records egress identically:
///
/// 1. Increments the per-tenant [`EGRESS_BYTES_TOTAL`] counter (the chargeback
///    source of truth, attributed by `(tenant, az_locality)`).
/// 2. When the locality is *chargeable* (cross-AZ / internet, never the free
///    same-AZ path), folds the bytes into the active per-query I/O trace's
///    `bytes_cross_az` term — the quantity the route cost model's egress weight
///    minimizes. Outside a query scope this silently no-ops.
///
/// Same-AZ bytes are metered (for visibility) but deliberately do NOT enter the
/// cost model, so the egress cost term stays inert on the free path.
pub fn record_egress_bytes(tenant_id: Option<&str>, locality: AzLocality, bytes: u64) {
    if bytes == 0 {
        return;
    }
    let t_id = tenant_id.unwrap_or("default");
    EGRESS_BYTES_TOTAL
        .with_label_values(&[t_id, locality.as_str()])
        .inc_by(bytes as f64);
    if locality.is_chargeable_egress() {
        crate::observability::io_trace::record_cross_az_bytes(bytes);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::io_trace;

    #[test]
    fn az_locality_strings_and_chargeability() {
        assert_eq!(AzLocality::SameAz.as_str(), "same_az");
        assert_eq!(AzLocality::CrossAz.as_str(), "cross_az");
        assert_eq!(AzLocality::Internet.as_str(), "internet");
        // The free path is not chargeable egress; the other two are.
        assert!(!AzLocality::SameAz.is_chargeable_egress());
        assert!(AzLocality::CrossAz.is_chargeable_egress());
        assert!(AzLocality::Internet.is_chargeable_egress());
    }

    #[test]
    fn az_locality_parse_defaults_to_free_path() {
        assert_eq!(AzLocality::parse("cross_az"), AzLocality::CrossAz);
        assert_eq!(AzLocality::parse("CROSS-AZ"), AzLocality::CrossAz);
        assert_eq!(AzLocality::parse("internet"), AzLocality::Internet);
        assert_eq!(AzLocality::parse("same_az"), AzLocality::SameAz);
        // Unknown / empty → the conservative free default (egress stays inert).
        assert_eq!(AzLocality::parse("nonsense"), AzLocality::SameAz);
        assert_eq!(AzLocality::parse(""), AzLocality::SameAz);
        assert_eq!(AzLocality::default(), AzLocality::SameAz);
    }

    #[tokio::test]
    async fn chargeable_egress_feeds_the_query_trace_but_same_az_does_not() {
        // Inside a query scope, only chargeable localities contribute to the
        // per-query bytes_cross_az that the cost model's egress term consumes.
        let snap = io_trace::scope(async {
            record_egress_bytes(Some("t"), AzLocality::CrossAz, 1_000);
            record_egress_bytes(Some("t"), AzLocality::Internet, 2_000);
            record_egress_bytes(Some("t"), AzLocality::SameAz, 9_000); // free → not traced
            record_egress_bytes(Some("t"), AzLocality::CrossAz, 0); // zero → ignored
            io_trace::snapshot()
        })
        .await
        .expect("snapshot inside scope");
        assert_eq!(snap.bytes_cross_az, 3_000);
    }

    #[tokio::test]
    async fn record_egress_outside_scope_is_a_noop() {
        // No active trace: the counter still records, but nothing panics and the
        // trace stays empty (helper silently no-ops the cross-AZ term).
        record_egress_bytes(Some("t"), AzLocality::CrossAz, 1_234);
        assert!(io_trace::snapshot().is_none());
    }
}
