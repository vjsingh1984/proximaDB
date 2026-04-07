//! Unified metrics helpers for query instrumentation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;
use tracing::{debug, info};

use crate::observability::ObservabilityService;
use crate::proto::proximadb_v1::{LogEntry, MetricSample, Severity, SqlValue, sql_value};

#[derive(Clone)]
struct QueryTelemetrySink {
    service: Arc<ObservabilityService>,
    namespace: String,
}

static QUERY_TELEMETRY_SINK: Lazy<Mutex<Option<QueryTelemetrySink>>> =
    Lazy::new(|| Mutex::new(None));

/// Configure the query telemetry sink with an observability service.
pub fn configure_query_telemetry(service: Arc<ObservabilityService>, namespace: impl Into<String>) {
    match QUERY_TELEMETRY_SINK.lock() {
        Ok(mut guard) => {
            *guard = Some(QueryTelemetrySink {
                service,
                namespace: namespace.into(),
            });
        }
        Err(e) => {
            tracing::error!(
                "Failed to configure query telemetry (mutex poisoned): {}",
                e
            );
        }
    }
}

/// Record the start of a query execution for telemetry.
pub fn record_query_start(kind: &str) {
    // Deferred: unify with proximadb_metrics once available in this crate scope
    info!("query_start" = kind);
    emit_query_log(kind, "start", true, None);
}

/// Record the end of a query execution with outcome and latency.
pub fn record_query_end(kind: &str, ok: bool, latency_ms: u64) {
    info!("query_end" = kind);
    emit_query_log(kind, "end", ok, Some(latency_ms));
    emit_query_metrics(kind, ok, latency_ms);
}

fn emit_query_log(kind: &str, stage: &str, ok: bool, latency_ms: Option<u64>) {
    let Some(sink) = current_sink() else {
        return;
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        debug!("query telemetry skipped: no active Tokio runtime");
        return;
    };

    let kind = kind.to_string();
    let stage = stage.to_string();
    handle.spawn(async move {
        let timestamp_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let severity = if ok { Severity::Info } else { Severity::Error };

        let mut fields = HashMap::new();
        fields.insert("kind".to_string(), string_value(kind.clone()));
        fields.insert("stage".to_string(), string_value(stage.clone()));
        fields.insert(
            "status".to_string(),
            string_value(if ok { "ok" } else { "error" }),
        );
        if let Some(latency_ms) = latency_ms {
            fields.insert("latency_ms".to_string(), int_value(latency_ms as i64));
        }

        let log = LogEntry {
            timestamp_ns,
            severity: severity as i32,
            message: format!("query.{}", stage),
            fields,
            source: Some("query-utils".to_string()),
            service: Some("proximadb".to_string()),
        };

        if let Err(error) = sink
            .service
            .ingest_logs(&sink.namespace, vec![log], None)
            .await
        {
            debug!("query telemetry log ingestion failed: {}", error);
        }
    });
}

fn emit_query_metrics(kind: &str, ok: bool, latency_ms: u64) {
    let Some(sink) = current_sink() else {
        return;
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        debug!("query telemetry metrics skipped: no active Tokio runtime");
        return;
    };

    let kind = kind.to_string();
    let status = if ok {
        "ok".to_string()
    } else {
        "error".to_string()
    };
    handle.spawn(async move {
        let timestamp_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let labels = HashMap::from([
            ("kind".to_string(), kind.clone()),
            ("status".to_string(), status.clone()),
        ]);
        let metrics = vec![
            MetricSample {
                name: "query_total".to_string(),
                timestamp_ns,
                value: 1.0,
                labels: labels.clone(),
            },
            MetricSample {
                name: "query_latency_ms".to_string(),
                timestamp_ns,
                value: latency_ms as f64,
                labels,
            },
        ];

        if let Err(error) = sink.service.ingest_metrics(&sink.namespace, metrics).await {
            debug!("query telemetry metric ingestion failed: {}", error);
        }
    });
}

fn current_sink() -> Option<QueryTelemetrySink> {
    match QUERY_TELEMETRY_SINK.lock() {
        Ok(guard) => guard.clone(),
        Err(e) => {
            tracing::error!(
                "Failed to access query telemetry sink (mutex poisoned): {}",
                e
            );
            None
        }
    }
}

fn string_value(value: impl Into<String>) -> SqlValue {
    SqlValue {
        value: Some(sql_value::Value::StringValue(value.into())),
    }
}

fn int_value(value: i64) -> SqlValue {
    SqlValue {
        value: Some(sql_value::Value::Int64Value(value)),
    }
}
