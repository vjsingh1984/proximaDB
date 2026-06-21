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

use std::sync::{LazyLock, Mutex};

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
    /// Per-tenant bytes moved OUT of the deployment — the **KOU** (Outgress Unit)
    /// meter (co-design Dimension 2 / §2.2). `direction` separates `read`
    /// (object store → compute) from `result` (compute → client); `kou_locality`
    /// is the cost discriminator (cross-region ~$0.02/GB, internet ~$0.09-0.12/GB
    /// by cloud; same-region/AZ free). Networking is frequently the dominant TCO
    /// term. Neutral telemetry only: the cloud topology and the $ weights are
    /// control-plane (anvaiops) policy. (KEU = embedding units is a separate meter.)
    pub static ref KOU_BYTES_TOTAL: CounterVec = registered_counter_vec(
        "proximadb_kou_bytes_total",
        "Outgress bytes per tenant by locality + direction (KOU, Dimension 2)",
        &["tenant_id", "kou_locality", "direction"]
    );
}

/// Cloud provider hosting the object store — the multi-cloud axis for egress
/// pricing (AWS / Azure / GCP each price data movement differently). OSS
/// **mechanism**: it is *inferred from the storage URL scheme* the operator
/// already configures (so it can never be set wrong), never hand-declared. The
/// $ weights per (provider, locality) are control-plane (anvaiops) **policy**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloudProvider {
    Aws,
    Azure,
    Gcp,
    /// Local filesystem / dev / unknown — no egress cost.
    #[default]
    Local,
}

impl CloudProvider {
    /// Stable label string.
    pub fn as_str(self) -> &'static str {
        match self {
            CloudProvider::Aws => "aws",
            CloudProvider::Azure => "azure",
            CloudProvider::Gcp => "gcp",
            CloudProvider::Local => "local",
        }
    }

    /// Infer the provider from an object-store URL scheme — the same scheme the
    /// `FilesystemFactory` dispatches on (`s3`, `gs`/`gcs`, `az`/`adls`/`abfs`,
    /// `file`). Unknown / empty → [`CloudProvider::Local`]. This is the
    /// mistake-proof seam: the provider follows the storage URL, not a separate
    /// env an operator could contradict.
    pub fn from_url_scheme(scheme: &str) -> Self {
        match scheme.trim().to_ascii_lowercase().as_str() {
            "s3" | "s3a" => CloudProvider::Aws,
            "gs" | "gcs" => CloudProvider::Gcp,
            "az" | "azure" | "adls" | "abfs" | "abfss" => CloudProvider::Azure,
            _ => CloudProvider::Local,
        }
    }
}

/// Locality of bytes moving out of the deployment — the **KOU** (Outgress Unit)
/// dimension (§2.2), cloud-neutral. Used for two flows: *read-egress* (object
/// store → compute) and *result-egress* (compute → client). Distinct from
/// **KEU = embedding units** (a separate, embedding-compute meter).
///
/// Region-granular by design: object-store reads are billed by *region*, not
/// availability zone — same-region access is **free on every cloud** (S3 via the
/// VPC gateway endpoint, ADLS/Blob is regional, GCS via private access). So
/// `Local`/`CrossAz` are recorded for visibility but **not** chargeable; only
/// `CrossRegion`/`CrossCloud`/`OnPrem`/`Internet` cost. `Unknown` is recorded and
/// surfaced (never silently charged or freed) so an operator fixes the scope map.
/// OSS records the neutral bucket; the per-(provider, bucket) $ weight is
/// anvaiops policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KouLocality {
    /// Compute and the other endpoint co-located (same region/zone) — free.
    /// The safe default so outgress stays inert until placement is known.
    #[default]
    Local,
    /// Same region, different availability zone — **free for object storage** on
    /// all clouds (recorded for visibility + the future compute↔compute meter).
    CrossAz,
    /// Different region, same cloud — chargeable cross-region transfer (~$0.02/GB).
    CrossRegion,
    /// Different cloud provider — chargeable (internet-tier, expensive).
    CrossCloud,
    /// To/from on-premises (over VPN / dedicated link / declared CIDR).
    OnPrem,
    /// Public internet (~$0.09-0.12/GB by cloud) — the dominant result-egress term.
    Internet,
    /// Could not be classified (no scope-map rule matched) — recorded + surfaced,
    /// never silently treated as free or charged.
    Unknown,
}

impl KouLocality {
    /// Stable metric-label string.
    pub fn as_str(self) -> &'static str {
        match self {
            KouLocality::Local => "local",
            KouLocality::CrossAz => "cross_az",
            KouLocality::CrossRegion => "cross_region",
            KouLocality::CrossCloud => "cross_cloud",
            KouLocality::OnPrem => "on_prem",
            KouLocality::Internet => "internet",
            KouLocality::Unknown => "unknown",
        }
    }

    /// Whether bytes through this locality are chargeable outgress. `Local` and
    /// `CrossAz` object-store access is free on every cloud, and `Unknown` is not
    /// charged (it is surfaced for the operator to fix), so only genuinely
    /// cross-boundary movement feeds the cost model's egress term + the trace.
    pub fn is_chargeable_egress(self) -> bool {
        matches!(
            self,
            KouLocality::CrossRegion
                | KouLocality::CrossCloud
                | KouLocality::OnPrem
                | KouLocality::Internet
        )
    }

    /// Parse an explicit operator override / scope-map value, accepting obvious
    /// aliases. `None` for an unrecognized value (caller keeps the derived value
    /// rather than silently mis-charging).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "local" | "same_az" | "colocated" | "same_zone" => Some(KouLocality::Local),
            "cross_az" | "same_region" | "region" => Some(KouLocality::CrossAz),
            "cross_region" | "crossregion" => Some(KouLocality::CrossRegion),
            "cross_cloud" | "crosscloud" => Some(KouLocality::CrossCloud),
            "on_prem" | "onprem" | "on_premises" => Some(KouLocality::OnPrem),
            "internet" | "egress" | "public" => Some(KouLocality::Internet),
            "unknown" => Some(KouLocality::Unknown),
            _ => None,
        }
    }

    /// Derive the **read-egress** locality (object store → compute) from a
    /// [`PlacementContext`] — the per-cloud truth table, the mistake-proof core.
    /// A different store cloud → `CrossCloud`; region equality (not zone) decides
    /// the rest; GCS multi-region scopes (`US`/`EU`/`ASIA`) containing the compute
    /// region count as same-region (`CrossAz`, free).
    pub fn derive_read(ctx: &PlacementContext) -> Self {
        // No cloud, or we don't know where compute runs → assume free (never
        // invent a charge); the caller logs the assumption.
        if ctx.store_provider == CloudProvider::Local || ctx.compute_region.trim().is_empty() {
            return KouLocality::Local;
        }
        // Compute in a different cloud than the store → cross-cloud read.
        if ctx.compute_provider != CloudProvider::Local
            && ctx.compute_provider != ctx.store_provider
        {
            return KouLocality::CrossCloud;
        }
        let compute = ctx.compute_region.trim().to_ascii_lowercase();
        let store = ctx.store_region.trim().to_ascii_lowercase();
        if store.is_empty() {
            return KouLocality::Local; // store region unknown → don't over-charge
        }
        if compute == store || gcs_multi_region_contains(&store, &compute) {
            // Same region (object store is regional) → free. Recorded as CrossAz
            // only when zones are known to differ; otherwise the free Local.
            KouLocality::Local
        } else {
            KouLocality::CrossRegion
        }
    }
}

/// Whether a GCS multi-region / dual-region bucket scope (`us`, `eu`, `asia`)
/// contains a specific compute region (e.g. `us-central1` ∈ `us`). Region-scoped
/// store values fall through to plain equality in the caller.
fn gcs_multi_region_contains(store: &str, compute: &str) -> bool {
    matches!(store, "us" | "eu" | "asia") && compute.starts_with(store)
}

/// Inputs to [`KouLocality::derive_read`]: the store cloud + region (from the
/// storage URL scheme + backend config) and where compute runs
/// (`PROXIMADB_COMPUTE_REGION`, control-plane stamped; `compute_provider`
/// defaults to the store provider — same cloud — unless declared otherwise).
#[derive(Debug, Clone, Default)]
pub struct PlacementContext {
    pub store_provider: CloudProvider,
    pub compute_provider: CloudProvider,
    pub compute_region: String,
    pub store_region: String,
}

/// The process-wide derived object-store egress locality, computed once at
/// startup by [`install_placement`]. Defaults to `Local` (free, inert) until the
/// deployment installs a real [`PlacementContext`] — so OSS-standalone / dev runs
/// never bill egress.
static OBJECT_STORE_LOCALITY: LazyLock<Mutex<KouLocality>> =
    LazyLock::new(|| Mutex::new(KouLocality::Local));

/// Install the deployment's placement (called once at startup). Derives the
/// read-egress locality from `ctx`, applies the `PROXIMADB_OBJECT_STORE_LOCALITY`
/// override when present+valid, stores it, and returns the effective value so the
/// caller can log it. A cloud provider with an unknown compute region logs a WARN
/// (visible assumption, not a silent free pass).
pub fn install_placement(ctx: &PlacementContext) -> KouLocality {
    let derived = KouLocality::derive_read(ctx);
    let effective = match std::env::var("PROXIMADB_OBJECT_STORE_LOCALITY") {
        Ok(v) if !v.trim().is_empty() => match KouLocality::parse(&v) {
            Some(override_loc) => override_loc,
            None => {
                tracing::warn!(
                    value = %v,
                    "ignoring invalid PROXIMADB_OBJECT_STORE_LOCALITY override; \
                     using derived locality"
                );
                derived
            }
        },
        _ => derived,
    };
    if ctx.store_provider != CloudProvider::Local && ctx.compute_region.trim().is_empty() {
        tracing::warn!(
            store_provider = ctx.store_provider.as_str(),
            "PROXIMADB_COMPUTE_REGION unset on a cloud deployment; assuming \
             same-region (free) egress — set it so cross-region egress is metered"
        );
    }
    tracing::info!(
        store_provider = ctx.store_provider.as_str(),
        compute_region = %ctx.compute_region,
        store_region = %ctx.store_region,
        locality = effective.as_str(),
        "co-design Dimension 2 (KOU): derived read-egress locality"
    );
    *OBJECT_STORE_LOCALITY
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = effective;
    effective
}

/// The effective object-store read-egress locality for this deployment (derived
/// at startup; `Local` until [`install_placement`] runs).
pub fn object_store_egress_locality() -> KouLocality {
    *OBJECT_STORE_LOCALITY
        .lock()
        .unwrap_or_else(|p| p.into_inner())
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

/// Record outgress bytes for one tenant — the single convergent **KOU** meter
/// (Dimension 2). `direction` is `"read"` (object store → compute) or `"result"`
/// (compute → client). It does two things at once so every boundary records
/// identically:
///
/// 1. Increments the per-tenant [`KOU_BYTES_TOTAL`] counter (the chargeback
///    source of truth, attributed by `(tenant, kou_locality, direction)`).
/// 2. When the locality is *chargeable* (cross-region/-cloud/on-prem/internet,
///    never the free local / cross-AZ / same-region path), folds the bytes into
///    the active per-query I/O trace's egress term — the quantity the route cost
///    model's egress weight minimizes. Outside a query scope this silently no-ops.
///
/// Free-path bytes are metered (for visibility) but deliberately do NOT enter the
/// cost model, so the egress cost term stays inert on the free path.
pub fn record_kou_bytes(
    tenant_id: Option<&str>,
    locality: KouLocality,
    direction: &str,
    bytes: u64,
) {
    if bytes == 0 {
        return;
    }
    let t_id = tenant_id.unwrap_or("default");
    KOU_BYTES_TOTAL
        .with_label_values(&[t_id, locality.as_str(), direction])
        .inc_by(bytes as f64);
    if locality.is_chargeable_egress() {
        crate::observability::io_trace::record_egress_bytes(bytes);
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
    fn provider_inferred_from_url_scheme() {
        assert_eq!(CloudProvider::from_url_scheme("s3"), CloudProvider::Aws);
        assert_eq!(CloudProvider::from_url_scheme("gs"), CloudProvider::Gcp);
        assert_eq!(CloudProvider::from_url_scheme("gcs"), CloudProvider::Gcp);
        assert_eq!(CloudProvider::from_url_scheme("abfs"), CloudProvider::Azure);
        assert_eq!(CloudProvider::from_url_scheme("adls"), CloudProvider::Azure);
        assert_eq!(CloudProvider::from_url_scheme("file"), CloudProvider::Local);
        assert_eq!(CloudProvider::from_url_scheme(""), CloudProvider::Local);
    }

    #[test]
    fn kou_locality_strings_and_chargeability() {
        assert_eq!(KouLocality::Local.as_str(), "local");
        assert_eq!(KouLocality::CrossAz.as_str(), "cross_az");
        assert_eq!(KouLocality::CrossRegion.as_str(), "cross_region");
        assert_eq!(KouLocality::CrossCloud.as_str(), "cross_cloud");
        assert_eq!(KouLocality::OnPrem.as_str(), "on_prem");
        assert_eq!(KouLocality::Internet.as_str(), "internet");
        assert_eq!(KouLocality::Unknown.as_str(), "unknown");
        // Local / cross-AZ object-store access is FREE on every cloud; Unknown is
        // surfaced, not charged. Only genuine cross-boundary movement is chargeable.
        assert!(!KouLocality::Local.is_chargeable_egress());
        assert!(!KouLocality::CrossAz.is_chargeable_egress());
        assert!(!KouLocality::Unknown.is_chargeable_egress());
        assert!(KouLocality::CrossRegion.is_chargeable_egress());
        assert!(KouLocality::CrossCloud.is_chargeable_egress());
        assert!(KouLocality::OnPrem.is_chargeable_egress());
        assert!(KouLocality::Internet.is_chargeable_egress());
        assert_eq!(KouLocality::default(), KouLocality::Local);
    }

    #[test]
    fn derive_read_is_region_granular_per_cloud() {
        let ctx = |p, c: &str, s: &str| PlacementContext {
            store_provider: p,
            compute_provider: p,
            compute_region: c.into(),
            store_region: s.into(),
        };
        // Same region → free (even if zones differ — object store is regional).
        assert_eq!(
            KouLocality::derive_read(&ctx(CloudProvider::Aws, "us-east-1", "us-east-1")),
            KouLocality::Local
        );
        // Different region → chargeable cross-region.
        assert_eq!(
            KouLocality::derive_read(&ctx(CloudProvider::Azure, "eastus", "westus")),
            KouLocality::CrossRegion
        );
        // GCS multi-region bucket "us" containing the compute region → free.
        assert_eq!(
            KouLocality::derive_read(&ctx(CloudProvider::Gcp, "us-central1", "us")),
            KouLocality::Local
        );
        assert_eq!(
            KouLocality::derive_read(&ctx(CloudProvider::Gcp, "europe-west1", "us")),
            KouLocality::CrossRegion
        );
        // Compute in a different cloud than the store → cross-cloud read.
        assert_eq!(
            KouLocality::derive_read(&PlacementContext {
                store_provider: CloudProvider::Gcp,
                compute_provider: CloudProvider::Aws,
                compute_region: "us-east-1".into(),
                store_region: "us-central1".into(),
            }),
            KouLocality::CrossCloud
        );
        // Unknown compute region or local provider → free default (never charge).
        assert_eq!(
            KouLocality::derive_read(&ctx(CloudProvider::Aws, "", "us-east-1")),
            KouLocality::Local
        );
        assert_eq!(
            KouLocality::derive_read(&ctx(CloudProvider::Local, "us-east-1", "us-east-1")),
            KouLocality::Local
        );
    }

    #[test]
    fn override_parse_rejects_unknown() {
        assert_eq!(
            KouLocality::parse("cross_region"),
            Some(KouLocality::CrossRegion)
        );
        assert_eq!(
            KouLocality::parse("CROSS-REGION"),
            Some(KouLocality::CrossRegion)
        );
        assert_eq!(
            KouLocality::parse("cross_cloud"),
            Some(KouLocality::CrossCloud)
        );
        assert_eq!(KouLocality::parse("on_prem"), Some(KouLocality::OnPrem));
        assert_eq!(KouLocality::parse("internet"), Some(KouLocality::Internet));
        // Unknown → None so the caller keeps the derived value (no silent mischarge).
        assert_eq!(KouLocality::parse("nonsense"), None);
        assert_eq!(KouLocality::parse(""), None);
    }

    #[tokio::test]
    async fn chargeable_egress_feeds_the_query_trace_but_free_path_does_not() {
        // Inside a query scope, only chargeable localities contribute to the
        // per-query egress term the cost model consumes.
        let snap = io_trace::scope(async {
            record_kou_bytes(Some("t"), KouLocality::CrossRegion, "read", 1_000);
            record_kou_bytes(Some("t"), KouLocality::Internet, "result", 2_000);
            record_kou_bytes(Some("t"), KouLocality::CrossAz, "read", 9_000); // free → not traced
            record_kou_bytes(Some("t"), KouLocality::Local, "read", 5_000); // free → not traced
            record_kou_bytes(Some("t"), KouLocality::Unknown, "result", 7_000); // surfaced, not charged
            record_kou_bytes(Some("t"), KouLocality::CrossRegion, "read", 0); // zero → ignored
            io_trace::snapshot()
        })
        .await
        .expect("snapshot inside scope");
        assert_eq!(snap.egress_bytes, 3_000);
    }

    #[tokio::test]
    async fn record_kou_outside_scope_is_a_noop() {
        // No active trace: the counter still records, but nothing panics and the
        // trace stays empty (helper silently no-ops the egress term).
        record_kou_bytes(Some("t"), KouLocality::CrossRegion, "read", 1_234);
        assert!(io_trace::snapshot().is_none());
    }
}
