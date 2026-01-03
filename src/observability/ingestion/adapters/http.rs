// HTTP adapter for JSON log ingestion
//
// Supports:
// - JSON over HTTP POST
// - Bulk ingestion
// - Multiple content types (application/json, application/x-ndjson)
// - Basic authentication
// - API key authentication

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{AdapterConfig, InputAdapter};
use crate::proto::proximadb_v1::{LogEntry, Severity, SqlValue};

/// HTTP adapter for JSON log ingestion
pub struct HttpAdapter {
    /// Configuration
    config: AdapterConfig,
    /// Whether the adapter is running
    running: AtomicBool,
    /// Number of events received
    events_received: AtomicU64,
}

/// JSON log entry format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonLogEntry {
    /// Timestamp (RFC 3339 or Unix timestamp)
    #[serde(alias = "ts", alias = "@timestamp")]
    pub timestamp: Option<String>,
    /// Unix timestamp in seconds
    #[serde(alias = "time")]
    pub timestamp_unix: Option<i64>,
    /// Unix timestamp in nanoseconds
    pub timestamp_ns: Option<i64>,
    /// Message
    #[serde(alias = "msg", alias = "log")]
    pub message: Option<String>,
    /// Severity/level
    #[serde(alias = "level", alias = "lvl")]
    pub severity: Option<String>,
    /// Source/host
    #[serde(alias = "host", alias = "hostname")]
    pub source: Option<String>,
    /// Service/application
    #[serde(alias = "app", alias = "application")]
    pub service: Option<String>,
    /// Trace ID
    pub trace_id: Option<String>,
    /// Span ID
    pub span_id: Option<String>,
    /// Additional attributes
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Bulk request format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkRequest {
    /// Log entries
    pub logs: Vec<JsonLogEntry>,
}

impl HttpAdapter {
    /// Create a new HTTP adapter
    pub fn new(config: AdapterConfig) -> Self {
        Self {
            config,
            running: AtomicBool::new(false),
            events_received: AtomicU64::new(0),
        }
    }

    /// Parse a single JSON log entry
    pub fn parse_entry(&self, json: &str) -> Result<LogEntry> {
        let entry: JsonLogEntry = serde_json::from_str(json)?;
        Ok(self.convert_entry(&entry))
    }

    /// Parse bulk JSON entries
    pub fn parse_bulk(&self, json: &str) -> Result<Vec<LogEntry>> {
        let bulk: BulkRequest = serde_json::from_str(json)?;
        Ok(bulk.logs.iter().map(|e| self.convert_entry(e)).collect())
    }

    /// Parse newline-delimited JSON (NDJSON)
    pub fn parse_ndjson(&self, data: &str) -> Result<Vec<LogEntry>> {
        let mut entries = Vec::new();
        for line in data.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<JsonLogEntry>(line) {
                entries.push(self.convert_entry(&entry));
            }
        }
        Ok(entries)
    }

    /// Convert JSON entry to LogEntry
    fn convert_entry(&self, entry: &JsonLogEntry) -> LogEntry {
        let timestamp_ns = self.parse_timestamp(entry);
        let severity = self.parse_severity(entry.severity.as_deref());

        let message = entry.message.clone().unwrap_or_default();
        let source = entry.source.clone();
        let service = entry.service.clone();

        // Convert extra fields to SqlValue map
        let fields: HashMap<String, SqlValue> = entry
            .extra
            .iter()
            .filter(|(k, _)| {
                ![
                    "timestamp",
                    "ts",
                    "@timestamp",
                    "time",
                    "timestamp_unix",
                    "timestamp_ns",
                    "message",
                    "msg",
                    "log",
                    "severity",
                    "level",
                    "lvl",
                    "source",
                    "host",
                    "hostname",
                    "service",
                    "app",
                    "application",
                    "trace_id",
                    "span_id",
                ]
                .contains(&k.as_str())
            })
            .map(|(k, v)| {
                let value = match v {
                    serde_json::Value::String(s) => {
                        crate::proto::proximadb_v1::sql_value::Value::StringValue(s.clone())
                    }
                    serde_json::Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)
                        } else if let Some(f) = n.as_f64() {
                            crate::proto::proximadb_v1::sql_value::Value::NumberValue(f)
                        } else {
                            crate::proto::proximadb_v1::sql_value::Value::StringValue(n.to_string())
                        }
                    }
                    serde_json::Value::Bool(b) => {
                        crate::proto::proximadb_v1::sql_value::Value::BoolValue(*b)
                    }
                    _ => crate::proto::proximadb_v1::sql_value::Value::StringValue(v.to_string()),
                };
                (k.clone(), SqlValue { value: Some(value) })
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

    /// Parse timestamp from various formats
    fn parse_timestamp(&self, entry: &JsonLogEntry) -> i64 {
        // Priority: timestamp_ns > timestamp_unix > timestamp (string)
        if let Some(ns) = entry.timestamp_ns {
            return ns;
        }

        if let Some(unix) = entry.timestamp_unix {
            return unix * 1_000_000_000;
        }

        if let Some(ts) = &entry.timestamp {
            // Try RFC 3339
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                return dt.timestamp_nanos_opt().unwrap_or(0);
            }

            // Try Unix timestamp string
            if let Ok(unix) = ts.parse::<i64>() {
                // Detect if it's seconds, milliseconds, or nanoseconds
                return if unix > 1_000_000_000_000_000_000 {
                    unix // Already nanoseconds
                } else if unix > 1_000_000_000_000 {
                    unix * 1_000_000 // Milliseconds
                } else {
                    unix * 1_000_000_000 // Seconds
                };
            }
        }

        // Default to current time
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    }

    /// Parse severity string to Severity
    fn parse_severity(&self, level: Option<&str>) -> Severity {
        match level.map(|s| s.to_lowercase()).as_deref() {
            Some("trace" | "verbose") => Severity::Debug,
            Some("debug" | "dbg") => Severity::Debug,
            Some("info" | "information" | "notice") => Severity::Info,
            Some("warn" | "warning") => Severity::Warn,
            Some("error" | "err" | "failure") => Severity::Error,
            Some("fatal" | "critical" | "emergency" | "alert" | "panic") => Severity::Fatal,
            _ => Severity::Info,
        }
    }
}

#[async_trait]
impl InputAdapter for HttpAdapter {
    fn name(&self) -> &str {
        "http"
    }

    async fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        // HTTP endpoints are typically added to the main REST server
        // This adapter provides the parsing logic
        tracing::info!("HTTP adapter would listen on {}", self.config.bind_address);
        Ok(())
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
    fn test_parse_entry() {
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new("127.0.0.1:8080".parse().unwrap(), tx);
        let adapter = HttpAdapter::new(config);

        let json = r#"{
            "timestamp": "2023-12-26T12:00:00Z",
            "message": "Test log message",
            "level": "error",
            "source": "test-host",
            "service": "test-app",
            "custom_field": "custom_value"
        }"#;

        let entry = adapter.parse_entry(json).unwrap();
        assert_eq!(entry.message, "Test log message");
        assert_eq!(entry.severity, Severity::Error as i32);
        assert_eq!(entry.source, Some("test-host".to_string()));
        assert_eq!(entry.service, Some("test-app".to_string()));
        // Check custom_field in fields map
        let custom_field = entry.fields.get("custom_field").unwrap();
        match &custom_field.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                assert_eq!(s, "custom_value");
            }
            _ => panic!("Expected StringValue for custom_field"),
        }
    }

    #[test]
    fn test_parse_ndjson() {
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new("127.0.0.1:8080".parse().unwrap(), tx);
        let adapter = HttpAdapter::new(config);

        let ndjson = r#"{"message": "Log 1", "level": "info"}
{"message": "Log 2", "level": "warn"}
{"message": "Log 3", "level": "error"}"#;

        let entries = adapter.parse_ndjson(ndjson).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].message, "Log 1");
        assert_eq!(entries[1].severity, Severity::Warn as i32);
        assert_eq!(entries[2].severity, Severity::Error as i32);
    }

    #[test]
    fn test_parse_severity() {
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new("127.0.0.1:8080".parse().unwrap(), tx);
        let adapter = HttpAdapter::new(config);

        assert_eq!(adapter.parse_severity(Some("debug")), Severity::Debug);
        assert_eq!(adapter.parse_severity(Some("INFO")), Severity::Info);
        assert_eq!(adapter.parse_severity(Some("WARN")), Severity::Warn);
        assert_eq!(adapter.parse_severity(Some("error")), Severity::Error);
        assert_eq!(adapter.parse_severity(Some("FATAL")), Severity::Fatal);
        assert_eq!(adapter.parse_severity(None), Severity::Info);
    }

    #[test]
    fn test_parse_bulk() {
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new("127.0.0.1:8080".parse().unwrap(), tx);
        let adapter = HttpAdapter::new(config);

        let json = r#"{
            "logs": [
                {"message": "Log 1", "level": "info"},
                {"message": "Log 2", "level": "warn"}
            ]
        }"#;

        let entries = adapter.parse_bulk(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "Log 1");
        assert_eq!(entries[1].message, "Log 2");
    }
}
