// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-161 external **OTLP metering push** — ships proximaDB's per-tenant billing
//! meters out to a standard OTLP collector (→ AnvaiOps), the *push* half of the
//! ADR-027 dual-sink (the durable [`crate::metrics::metering_writer`] is the
//! *durable-file* half).
//!
//! **Direction.** This is *export* of proximaDB's own meters, distinct from the
//! OTLP *ingestion* adapters (`crate::observability::ingestion`) that receive
//! third-party telemetry *into* proximaDB. Ingestion reuses proximaDB's own proto
//! over the existing `tonic`; export must be wire-compatible with arbitrary
//! standard OTLP collectors, so it uses the official OTLP schema via the
//! opentelemetry SDK.
//!
//! **Two gates (compiled-in, runtime-inert).** The capability is first-class — it
//! is in the default build — but:
//!   * the `otlp-metering` cargo feature lets a size-sensitive `--no-default-features`
//!     build drop the opentelemetry transitive surface entirely (this module then
//!     compiles to no-ops); and
//!   * at runtime it stays **inert until `PROXIMADB_OTLP_ENDPOINT` is set** — no
//!     connection, no exporter, zero cost — which is also the repo's
//!     "ship default-OFF until baked" stance (the behaviour is off by default; the
//!     code is present).
//!
//! **Standard outside.** Per the co-design mandate, the meters are vertical
//! (proximaDB's own KSU/KRU/… semantics) but the *seam* is the OTLP standard, so
//! any collector (Grafana/Datadog/AnvaiOps) reads it without bespoke glue.

use crate::metrics::consumption_metrics::TenantStorageUsage;

#[cfg(feature = "otlp-metering")]
pub use imp::{init_from_env, record_storage_snapshot, shutdown};

/// No-op when the `otlp-metering` feature is compiled out — callers (startup +
/// the metering daemon) stay feature-agnostic.
#[cfg(not(feature = "otlp-metering"))]
pub fn init_from_env() {}

/// No-op when the `otlp-metering` feature is compiled out.
#[cfg(not(feature = "otlp-metering"))]
pub fn record_storage_snapshot(_usage: &[TenantStorageUsage]) {}

/// No-op when the `otlp-metering` feature is compiled out.
#[cfg(not(feature = "otlp-metering"))]
pub fn shutdown() {}

#[cfg(feature = "otlp-metering")]
mod imp {
    use super::TenantStorageUsage;
    use std::sync::OnceLock;

    use opentelemetry::KeyValue;
    use opentelemetry::metrics::{Gauge, MeterProvider as _};
    use opentelemetry_otlp::{MetricExporter, WithExportConfig};
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use opentelemetry_sdk::metrics::periodic_reader_with_async_runtime::PeriodicReader;
    use opentelemetry_sdk::runtime;

    struct OtlpMetering {
        /// Held for its lifetime: dropping the provider flushes and stops the
        /// background export reader, so it must outlive the process.
        provider: SdkMeterProvider,
        /// Per-tenant resident-storage (KSU) level. Synchronous last-value gauge:
        /// the daemon `record`s the current level each snapshot; the reader
        /// re-exports it on its own interval (correct for a slow-moving stock).
        storage_gauge: Gauge<u64>,
    }

    /// Resolved exactly once. `Some` = pushing; `None` = inert (no endpoint, or
    /// build failed — logged once, never retried, never fatal).
    static EXPORTER: OnceLock<Option<OtlpMetering>> = OnceLock::new();

    /// Initialize the OTLP metering exporter from the environment. Idempotent
    /// (first call wins). Must run inside the tokio runtime — the grpc-tonic
    /// exporter's reader is driven on it.
    pub fn init_from_env() {
        EXPORTER.get_or_init(build_from_env);
    }

    fn build_from_env() -> Option<OtlpMetering> {
        let endpoint = std::env::var("PROXIMADB_OTLP_ENDPOINT")
            .ok()
            .filter(|e| !e.is_empty())?; // unset → inert (the expected default)
        match build(&endpoint) {
            Ok(m) => {
                tracing::info!("TD-161: OTLP metering push enabled → {endpoint}");
                Some(m)
            }
            Err(e) => {
                tracing::warn!(
                    "TD-161: OTLP metering push misconfigured ({endpoint}), disabled: {e}"
                );
                None
            }
        }
    }

    fn build(endpoint: &str) -> anyhow::Result<OtlpMetering> {
        let exporter = MetricExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .map_err(|e| anyhow::anyhow!("build OTLP metric exporter: {e}"))?;
        // rt-tokio reader: export() makes a tonic gRPC call and needs a tokio
        // reactor — the default thread-based reader would block_on it with no
        // runtime and panic. `runtime::Tokio` drives it on the server's runtime.
        let reader = PeriodicReader::builder(exporter, runtime::Tokio).build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("proximadb.metering");
        let storage_gauge = meter
            .u64_gauge("proximadb.ksu.resident_bytes")
            .with_unit("By")
            .with_description("Per-tenant resident storage bytes (KSU accrual level)")
            .build();
        Ok(OtlpMetering {
            provider,
            storage_gauge,
        })
    }

    /// Record one KSU storage snapshot to the OTLP gauge, keyed by tenant. No-op
    /// when inert. Fed from the *same* [`TenantStorageUsage`] the durable writer
    /// persists, so the pushed level and the durable record cannot diverge.
    pub fn record_storage_snapshot(usage: &[TenantStorageUsage]) {
        if let Some(Some(m)) = EXPORTER.get() {
            for u in usage {
                m.storage_gauge.record(
                    u.resident_bytes,
                    &[KeyValue::new("tenant_id", u.tenant_id.clone())],
                );
            }
        }
    }

    /// Flush and stop the exporter (best-effort) on graceful shutdown.
    pub fn shutdown() {
        if let Some(Some(m)) = EXPORTER.get()
            && let Err(e) = m.provider.shutdown()
        {
            tracing::warn!("TD-161: OTLP metering shutdown error: {e}");
        }
    }
}
