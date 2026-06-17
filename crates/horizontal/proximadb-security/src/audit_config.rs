//! Shared audit configuration contracts.
//!
//! Runtime logger construction, file IO, database writers, and webhook
//! dispatch remain in root/platform modules. This module only owns serializable
//! configuration DTOs that security, networking, and runtime composition can
//! share without depending on concrete audit implementations.

use serde::{Deserialize, Serialize};

/// Configuration for audit logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Master switch to enable or disable all audit logging.
    pub enable_audit_logging: bool,
    /// Backend storage target for persisting audit events.
    pub storage_backend: AuditStorageBackend,
    /// When true, sensitive fields (IP, user agent, secrets) are encrypted before storage.
    pub encryption_enabled: bool,
    /// Optional HTTP endpoint to which audit events are forwarded in real time.
    pub external_audit_endpoint: Option<String>,
    /// Number of days to retain audit log files before automatic deletion.
    pub retention_days: u32,
    /// When true, security alerts are dispatched in real time via webhook or email.
    pub enable_real_time_alerts: bool,
    /// Optional webhook URL for delivering real-time security alert payloads.
    pub alert_webhook_url: Option<String>,
    /// Compliance frameworks to tag against audit events (e.g., "SOC2", "GDPR", "HIPAA").
    pub compliance_frameworks: Vec<String>,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enable_audit_logging: true,
            storage_backend: AuditStorageBackend::File {
                directory: "/tmp/proximadb/audit".to_string(),
            },
            encryption_enabled: false,
            external_audit_endpoint: None,
            retention_days: 30,
            enable_real_time_alerts: false,
            alert_webhook_url: None,
            compliance_frameworks: vec![],
        }
    }
}

/// Audit storage backend options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditStorageBackend {
    /// Store audit logs as JSONL files in the specified local directory.
    File {
        /// Absolute path to the directory where audit log files are written.
        directory: String,
    },
    /// Store audit events in a relational database.
    Database {
        /// Database connection string (e.g., "postgres://user:pass@host/db").
        connection_string: String,
    },
    /// Store audit logs in an Amazon S3 bucket.
    S3 {
        /// S3 bucket name.
        bucket: String,
        /// AWS region where the bucket resides (e.g., "us-east-1").
        region: String,
    },
    /// Write to a primary backend and mirror to a secondary backend.
    Combined {
        /// Primary storage backend that receives all writes first.
        primary: Box<AuditStorageBackend>,
        /// Secondary (mirror) storage backend for redundancy.
        secondary: Box<AuditStorageBackend>,
    },
}

/// Supported encryption algorithms for audit payload protection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EncryptionAlgorithm {
    /// AES-256 in Galois/Counter Mode (authenticated encryption).
    AES256GCM,
    /// ChaCha20 stream cipher combined with Poly1305 MAC (authenticated encryption).
    ChaCha20Poly1305,
}

/// Email configuration for alert notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    /// Hostname or IP address of the SMTP relay server.
    pub smtp_server: String,
    /// TCP port of the SMTP relay server (typically 25, 465, or 587).
    pub smtp_port: u16,
    /// SMTP authentication username.
    pub username: String,
    /// SMTP authentication password.
    pub password: String,
    /// Email address that appears in the "From" header of alert messages.
    pub from_address: String,
    /// List of email addresses that receive alert notifications.
    pub to_addresses: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_config_default_preserves_existing_file_backend() {
        let config = AuditConfig::default();

        assert!(config.enable_audit_logging);
        assert!(!config.encryption_enabled);
        assert_eq!(config.retention_days, 30);
        assert!(matches!(
            config.storage_backend,
            AuditStorageBackend::File { .. }
        ));
    }

    #[test]
    fn audit_config_round_trips_through_json() {
        let config = AuditConfig {
            retention_days: 90,
            compliance_frameworks: vec!["SOC2".to_string(), "GDPR".to_string()],
            ..AuditConfig::default()
        };

        let json = serde_json::to_string(&config).expect("serialize audit config");
        let decoded: AuditConfig = serde_json::from_str(&json).expect("deserialize audit config");

        assert_eq!(decoded.retention_days, 90);
        assert_eq!(decoded.compliance_frameworks, ["SOC2", "GDPR"]);
    }

    #[test]
    fn combined_storage_backend_keeps_primary_and_secondary() {
        let backend = AuditStorageBackend::Combined {
            primary: Box::new(AuditStorageBackend::File {
                directory: "/tmp/audit".to_string(),
            }),
            secondary: Box::new(AuditStorageBackend::S3 {
                bucket: "audit-bucket".to_string(),
                region: "us-east-1".to_string(),
            }),
        };

        let AuditStorageBackend::Combined { primary, secondary } = backend else {
            panic!("expected combined backend");
        };

        assert!(matches!(*primary, AuditStorageBackend::File { .. }));
        assert!(matches!(*secondary, AuditStorageBackend::S3 { .. }));
    }

    #[test]
    fn email_config_is_serializable_contract() {
        let config = EmailConfig {
            smtp_server: "smtp.example.com".to_string(),
            smtp_port: 587,
            username: "user".to_string(),
            password: "secret".to_string(),
            from_address: "alerts@example.com".to_string(),
            to_addresses: vec!["ops@example.com".to_string()],
        };

        let json = serde_json::to_string(&config).expect("serialize email config");
        let decoded: EmailConfig = serde_json::from_str(&json).expect("deserialize email config");

        assert_eq!(decoded.smtp_server, "smtp.example.com");
        assert_eq!(decoded.smtp_port, 587);
        assert_eq!(decoded.to_addresses, ["ops@example.com"]);
    }
}
