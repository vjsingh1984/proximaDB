// Fluent adapter (Fluent Bit/Fluentd forward protocol)
//
// # INCOMPLETE - DO NOT USE IN PRODUCTION
//
// This adapter is incomplete. The `parse_forward_message` function returns
// empty results because MessagePack parsing is not implemented.
//
// ## Status
// - TCP listener: Works (accepts connections)
// - MessagePack parsing: NOT IMPLEMENTED (TODO in code)
// - Log conversion: Implemented but never called due to parsing issue
//
// ## Recommended Alternative
//
// For Fluent Bit/Fluentd integration, use Fluent Bit's HTTP output
// with ProximaDB's JSON/HTTP ingestion endpoint instead:
//
// ```ini
// [OUTPUT]
//     Name        http
//     Match       *
//     Host        localhost
//     Port        5678
//     URI         /v1/observability/logs
//     Format      json
// ```
//
// ---
//
// ## Legacy Documentation (Archived)
//
// Supports:
// - Forward protocol (MessagePack over TCP)
// - Secure forward (TLS)
// - Shared key authentication

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

use super::{AdapterConfig, InputAdapter};
use crate::proto::proximadb_v1::{LogEntry, Severity, SqlValue};

/// Fluent adapter for Fluent Bit/Fluentd forward protocol
pub struct FluentAdapter {
    /// Configuration
    config: AdapterConfig,
    /// Whether the adapter is running
    running: AtomicBool,
    /// Number of events received
    events_received: AtomicU64,
}

impl FluentAdapter {
    /// Create a new Fluent adapter
    pub fn new(config: AdapterConfig) -> Self {
        Self {
            config,
            running: AtomicBool::new(false),
            events_received: AtomicU64::new(0),
        }
    }

    /// Parse MessagePack-encoded forward message
    fn parse_forward_message(&self, _data: &[u8]) -> Result<Vec<LogEntry>> {
        let entries = Vec::new();

        // Forward protocol format:
        // [tag, [[time, record], [time, record], ...]]
        // or
        // [tag, time, record] for single events

        // TODO: Implement full MessagePack parsing using rmp-serde
        // For now, return empty as placeholder

        Ok(entries)
    }

    /// Convert fluent record to LogEntry
    fn convert_record(
        &self,
        tag: &str,
        timestamp_sec: i64,
        record: &HashMap<String, serde_json::Value>,
    ) -> LogEntry {
        let timestamp_ns = timestamp_sec * 1_000_000_000;

        let message = record
            .get("message")
            .or_else(|| record.get("log"))
            .or_else(|| record.get("msg"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let severity = record
            .get("level")
            .or_else(|| record.get("severity"))
            .and_then(|v| v.as_str())
            .map(|s| self.parse_level(s))
            .unwrap_or(Severity::Info);

        let source = record
            .get("host")
            .or_else(|| record.get("hostname"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let service = record
            .get("service")
            .or_else(|| record.get("app"))
            .or_else(|| record.get("application"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some(tag.to_string()));

        // Convert remaining fields to SqlValue map
        let fields: HashMap<String, SqlValue> = record
            .iter()
            .filter(|(k, _)| {
                ![
                    "message",
                    "log",
                    "msg",
                    "level",
                    "severity",
                    "host",
                    "hostname",
                    "service",
                    "app",
                    "application",
                    "time",
                    "timestamp",
                ]
                .contains(&k.as_str())
            })
            .map(|(k, v)| {
                (
                    k.clone(),
                    SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                            v.to_string(),
                        )),
                    },
                )
            })
            .collect();

        LogEntry {
            timestamp_ns,
            severity: severity as i32,
            message,
            fields,
            source,
            service,
        }
    }

    /// Parse log level string to severity
    fn parse_level(&self, level: &str) -> Severity {
        match level.to_lowercase().as_str() {
            "trace" | "verbose" => Severity::Debug,
            "debug" => Severity::Debug,
            "info" | "information" => Severity::Info,
            "warn" | "warning" => Severity::Warn,
            "error" | "err" => Severity::Error,
            "fatal" | "critical" | "emergency" | "alert" => Severity::Fatal,
            _ => Severity::Info,
        }
    }

    /// Start TCP listener for forward protocol
    async fn start_listener(&self) -> Result<()> {
        let listener = TcpListener::bind(self.config.bind_address)
            .await
            .context("Failed to bind Fluent listener")?;

        let _sender = self.config.sender.clone();
        let batch_size = self.config.batch_size;
        let running = Arc::new(AtomicBool::new(true));
        let events = Arc::new(AtomicU64::new(0));

        tokio::spawn(async move {
            let _batch: Vec<crate::proto::proximadb_v1::LogEntry> = Vec::with_capacity(batch_size);

            while running.load(Ordering::Relaxed) {
                match listener.accept().await {
                    Ok((mut stream, _addr)) => {
                        let mut buf = Vec::new();

                        // Read MessagePack data
                        loop {
                            let mut chunk = [0u8; 4096];
                            match stream.read(&mut chunk).await {
                                Ok(0) => break,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                                Err(_) => break,
                            }
                        }

                        // TODO: Parse MessagePack and convert to LogEntry
                        // For now, just log receipt
                        events.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => continue,
                }
            }
        });

        Ok(())
    }
}

#[async_trait]
impl InputAdapter for FluentAdapter {
    fn name(&self) -> &str {
        "fluent"
    }

    async fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        self.start_listener().await
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
    use tokio::sync::mpsc;

    #[test]
    fn test_parse_level() {
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new("127.0.0.1:24224".parse().unwrap(), tx);
        let adapter = FluentAdapter::new(config);

        assert_eq!(adapter.parse_level("trace"), Severity::Debug);
        assert_eq!(adapter.parse_level("DEBUG"), Severity::Debug);
        assert_eq!(adapter.parse_level("info"), Severity::Info);
        assert_eq!(adapter.parse_level("WARN"), Severity::Warn);
        assert_eq!(adapter.parse_level("error"), Severity::Error);
        assert_eq!(adapter.parse_level("FATAL"), Severity::Fatal);
    }
}
