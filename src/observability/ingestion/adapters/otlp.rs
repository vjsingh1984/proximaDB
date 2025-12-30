// OTLP adapter (OpenTelemetry Protocol)
//
// Supports:
// - gRPC transport (port 4317)
// - HTTP/JSON transport (port 4318)
// - Logs, metrics, and traces

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use super::{AdapterConfig, InputAdapter};
use crate::proto::proximadb_v1::{LogEntry, Severity, SqlValue};

/// OTLP adapter for OpenTelemetry protocol
pub struct OtlpAdapter {
    /// Configuration
    config: AdapterConfig,
    /// Whether the adapter is running
    running: AtomicBool,
    /// Number of events received
    events_received: AtomicU64,
    /// Transport type
    transport: OtlpTransport,
}

/// OTLP transport type
#[derive(Debug, Clone, Copy)]
pub enum OtlpTransport {
    /// gRPC transport (default OTLP port 4317)
    Grpc,
    /// HTTP/JSON transport (default OTLP port 4318)
    Http,
}

impl OtlpAdapter {
    /// Create a new OTLP adapter
    pub fn new(config: AdapterConfig, transport: OtlpTransport) -> Self {
        Self {
            config,
            running: AtomicBool::new(false),
            events_received: AtomicU64::new(0),
            transport,
        }
    }

    /// Convert OTLP severity to our Severity
    fn convert_severity(severity_number: i32) -> Severity {
        // OTLP severity numbers: 1-4 = TRACE, 5-8 = DEBUG, 9-12 = INFO,
        // 13-16 = WARN, 17-20 = ERROR, 21-24 = FATAL
        match severity_number {
            1..=4 => Severity::Debug,   // TRACE
            5..=8 => Severity::Debug,   // DEBUG
            9..=12 => Severity::Info,   // INFO
            13..=16 => Severity::Warn,  // WARN
            17..=20 => Severity::Error, // ERROR
            21..=24 => Severity::Fatal, // FATAL
            _ => Severity::Info,
        }
    }

    /// Convert OTLP log record to LogEntry
    fn convert_log_record(
        &self,
        timestamp_ns: i64,
        severity_number: i32,
        body: &str,
        attributes: HashMap<String, String>,
        resource_attributes: &HashMap<String, String>,
    ) -> LogEntry {
        let source = resource_attributes
            .get("host.name")
            .cloned();
        let service = resource_attributes
            .get("service.name")
            .cloned();

        // Convert attributes to SqlValue map
        let fields: HashMap<String, SqlValue> = attributes
            .into_iter()
            .map(|(k, v)| (k, SqlValue { value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(v)) }))
            .collect();

        LogEntry {
            timestamp_ns,
            severity: Self::convert_severity(severity_number) as i32,
            message: body.to_string(),
            fields,
            source,
            service,
        }
    }

    /// Start gRPC server
    async fn start_grpc(&self) -> Result<()> {
        // TODO: Implement gRPC server for OTLP
        // This would use tonic to implement the OTLP LogsService, MetricsService, TraceService
        tracing::info!(
            "OTLP gRPC adapter would listen on {}",
            self.config.bind_address
        );
        Ok(())
    }

    /// Start HTTP server
    async fn start_http(&self) -> Result<()> {
        // TODO: Implement HTTP server for OTLP
        // This would accept POST requests to /v1/logs, /v1/metrics, /v1/traces
        tracing::info!(
            "OTLP HTTP adapter would listen on {}",
            self.config.bind_address
        );
        Ok(())
    }
}

#[async_trait]
impl InputAdapter for OtlpAdapter {
    fn name(&self) -> &str {
        match self.transport {
            OtlpTransport::Grpc => "otlp-grpc",
            OtlpTransport::Http => "otlp-http",
        }
    }

    async fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        match self.transport {
            OtlpTransport::Grpc => self.start_grpc().await,
            OtlpTransport::Http => self.start_http().await,
        }
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn events_received(&self) -> u64 {
        self.events_received.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_severity() {
        assert_eq!(OtlpAdapter::convert_severity(1), Severity::Debug);  // TRACE1
        assert_eq!(OtlpAdapter::convert_severity(5), Severity::Debug);  // DEBUG1
        assert_eq!(OtlpAdapter::convert_severity(9), Severity::Info);   // INFO1
        assert_eq!(OtlpAdapter::convert_severity(13), Severity::Warn);  // WARN1
        assert_eq!(OtlpAdapter::convert_severity(17), Severity::Error); // ERROR1
        assert_eq!(OtlpAdapter::convert_severity(21), Severity::Fatal); // FATAL1
    }
}
