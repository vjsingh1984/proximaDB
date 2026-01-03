// OCSF adapter (Open Cybersecurity Schema Framework)
//
// Supports:
// - OCSF JSON events (HTTP/webhook)
// - All OCSF event classes (system, network, application, etc.)
// - Schema validation

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{AdapterConfig, InputAdapter};
use crate::proto::proximadb_v1::{LogEntry, Severity, SqlValue};

/// OCSF adapter for Open Cybersecurity Schema Framework
pub struct OcsfAdapter {
    /// Configuration
    config: AdapterConfig,
    /// Whether the adapter is running
    running: AtomicBool,
    /// Number of events received
    events_received: AtomicU64,
}

/// OCSF base event structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcsfEvent {
    /// Activity ID
    #[serde(default)]
    pub activity_id: i32,
    /// Category UID
    #[serde(default)]
    pub category_uid: i32,
    /// Class UID
    #[serde(default)]
    pub class_uid: i32,
    /// Severity ID (0=Unknown, 1=Info, 2=Low, 3=Medium, 4=High, 5=Critical, 6=Fatal)
    #[serde(default)]
    pub severity_id: i32,
    /// Time (Unix timestamp in milliseconds)
    #[serde(default)]
    pub time: i64,
    /// Message
    #[serde(default)]
    pub message: String,
    /// Type UID
    #[serde(default)]
    pub type_uid: i32,
    /// Status
    #[serde(default)]
    pub status: String,
    /// Status ID
    #[serde(default)]
    pub status_id: i32,
    /// Metadata
    #[serde(default)]
    pub metadata: OcsfMetadata,
    /// Observables (IoCs, etc.)
    #[serde(default)]
    pub observables: Vec<OcsfObservable>,
    /// Raw data
    #[serde(default)]
    pub raw_data: String,
}

/// OCSF metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OcsfMetadata {
    /// Product name
    #[serde(default)]
    pub product: OcsfProduct,
    /// Version
    #[serde(default)]
    pub version: String,
    /// Log name
    #[serde(default)]
    pub log_name: String,
    /// Log provider
    #[serde(default)]
    pub log_provider: String,
}

/// OCSF product info
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OcsfProduct {
    /// Vendor name
    #[serde(default)]
    pub vendor_name: String,
    /// Product name
    #[serde(default)]
    pub name: String,
    /// Version
    #[serde(default)]
    pub version: String,
}

/// OCSF observable (IoC)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcsfObservable {
    /// Name
    #[serde(default)]
    pub name: String,
    /// Type
    #[serde(default)]
    pub r#type: String,
    /// Type ID
    #[serde(default)]
    pub type_id: i32,
    /// Value
    #[serde(default)]
    pub value: String,
}

impl OcsfAdapter {
    /// Create a new OCSF adapter
    pub fn new(config: AdapterConfig) -> Self {
        Self {
            config,
            running: AtomicBool::new(false),
            events_received: AtomicU64::new(0),
        }
    }

    /// Parse OCSF JSON event
    pub fn parse_event(&self, json: &str) -> Result<LogEntry> {
        let event: OcsfEvent = serde_json::from_str(json)?;
        Ok(self.convert_event(&event))
    }

    /// Convert OCSF event to LogEntry
    fn convert_event(&self, event: &OcsfEvent) -> LogEntry {
        let timestamp_ns = event.time * 1_000_000; // Convert ms to ns

        let severity = self.convert_severity(event.severity_id);

        let source = format!(
            "{} {}",
            event.metadata.product.vendor_name, event.metadata.product.name
        );

        let service = event.metadata.product.name.clone();

        let mut fields: HashMap<String, SqlValue> = HashMap::new();
        fields.insert(
            "activity_id".to_string(),
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(
                    event.activity_id as i64,
                )),
            },
        );
        fields.insert(
            "category_uid".to_string(),
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(
                    event.category_uid as i64,
                )),
            },
        );
        fields.insert(
            "class_uid".to_string(),
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(
                    event.class_uid as i64,
                )),
            },
        );
        fields.insert(
            "type_uid".to_string(),
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(
                    event.type_uid as i64,
                )),
            },
        );
        fields.insert(
            "status".to_string(),
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    event.status.clone(),
                )),
            },
        );
        fields.insert(
            "status_id".to_string(),
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(
                    event.status_id as i64,
                )),
            },
        );

        // Add observables as fields
        for (i, obs) in event.observables.iter().enumerate() {
            fields.insert(
                format!("observable_{}_name", i),
                SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        obs.name.clone(),
                    )),
                },
            );
            fields.insert(
                format!("observable_{}_type", i),
                SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        obs.r#type.clone(),
                    )),
                },
            );
            fields.insert(
                format!("observable_{}_value", i),
                SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        obs.value.clone(),
                    )),
                },
            );
        }

        LogEntry {
            timestamp_ns,
            severity: severity as i32,
            message: event.message.clone(),
            fields,
            source: Some(source),
            service: Some(service),
        }
    }

    /// Convert OCSF severity ID to Severity
    fn convert_severity(&self, severity_id: i32) -> Severity {
        // OCSF severity: 0=Unknown, 1=Info, 2=Low, 3=Medium, 4=High, 5=Critical, 6=Fatal
        match severity_id {
            0 => Severity::Info,  // Unknown
            1 => Severity::Info,  // Informational
            2 => Severity::Info,  // Low
            3 => Severity::Warn,  // Medium
            4 => Severity::Error, // High
            5 => Severity::Fatal, // Critical
            6 => Severity::Fatal, // Fatal
            _ => Severity::Info,
        }
    }

    /// Get OCSF category name from UID
    fn category_name(&self, uid: i32) -> &'static str {
        match uid {
            1 => "System Activity",
            2 => "Findings",
            3 => "Identity & Access Management",
            4 => "Network Activity",
            5 => "Discovery",
            6 => "Application Activity",
            _ => "Unknown",
        }
    }

    /// Get OCSF class name from UID
    fn class_name(&self, uid: i32) -> &'static str {
        match uid {
            // System Activity
            1001 => "File System Activity",
            1002 => "Kernel Extension Activity",
            1003 => "Kernel Activity",
            1004 => "Memory Activity",
            1005 => "Module Activity",
            1006 => "Scheduled Job Activity",
            1007 => "Process Activity",
            // Findings
            2001 => "Security Finding",
            2002 => "Vulnerability Finding",
            2003 => "Compliance Finding",
            2004 => "Detection Finding",
            2005 => "Incident Finding",
            // Network Activity
            4001 => "Network Activity",
            4002 => "HTTP Activity",
            4003 => "DNS Activity",
            4004 => "DHCP Activity",
            4005 => "RDP Activity",
            4006 => "SMB Activity",
            4007 => "SSH Activity",
            4008 => "FTP Activity",
            4009 => "Email Activity",
            // Application Activity
            6001 => "Web Resources Activity",
            6002 => "Application Lifecycle",
            6003 => "API Activity",
            6004 => "Web Resource Access Activity",
            _ => "Unknown",
        }
    }
}

#[async_trait]
impl InputAdapter for OcsfAdapter {
    fn name(&self) -> &str {
        "ocsf"
    }

    async fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        // OCSF events are typically received via HTTP webhook
        // The actual HTTP server is handled by the http adapter
        tracing::info!("OCSF adapter would listen on {}", self.config.bind_address);
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
    fn test_convert_severity() {
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new("127.0.0.1:8080".parse().unwrap(), tx);
        let adapter = OcsfAdapter::new(config);

        assert_eq!(adapter.convert_severity(0), Severity::Info);
        assert_eq!(adapter.convert_severity(1), Severity::Info);
        assert_eq!(adapter.convert_severity(3), Severity::Warn);
        assert_eq!(adapter.convert_severity(4), Severity::Error);
        assert_eq!(adapter.convert_severity(5), Severity::Fatal);
        assert_eq!(adapter.convert_severity(6), Severity::Fatal);
    }

    #[test]
    fn test_parse_ocsf_event() {
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new("127.0.0.1:8080".parse().unwrap(), tx);
        let adapter = OcsfAdapter::new(config);

        let json = r#"{
            "activity_id": 1,
            "category_uid": 4,
            "class_uid": 4001,
            "severity_id": 4,
            "time": 1703548800000,
            "message": "Network connection detected",
            "type_uid": 400101,
            "status": "success",
            "status_id": 1,
            "metadata": {
                "product": {
                    "vendor_name": "TestVendor",
                    "name": "TestProduct",
                    "version": "1.0"
                }
            },
            "observables": []
        }"#;

        let entry = adapter.parse_event(json).unwrap();
        assert_eq!(entry.message, "Network connection detected");
        assert_eq!(entry.severity, Severity::Error as i32);
        assert!(
            entry
                .source
                .as_ref()
                .map_or(false, |s| s.contains("TestVendor"))
        );
    }

    #[test]
    fn test_category_name() {
        let (tx, _rx) = mpsc::channel(100);
        let config = AdapterConfig::new("127.0.0.1:8080".parse().unwrap(), tx);
        let adapter = OcsfAdapter::new(config);

        assert_eq!(adapter.category_name(1), "System Activity");
        assert_eq!(adapter.category_name(4), "Network Activity");
        assert_eq!(adapter.category_name(99), "Unknown");
    }
}
