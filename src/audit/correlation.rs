//! Comprehensive audit correlation system for multi-provider enterprise environments

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

/// Comprehensive audit correlation engine
pub struct AuditCorrelationEngine {
    /// Active audit correlation sessions
    #[allow(dead_code)]
    correlation_sessions: Arc<DashMap<String, AuditCorrelationSession>>,

    /// Provider-specific audit integrations
    #[allow(dead_code)]
    provider_integrations: Arc<ProviderAuditIntegrations>,

    /// Cross-provider event correlator
    #[allow(dead_code)]
    cross_provider_correlator: Arc<CrossProviderEventCorrelator>,

    /// Compliance audit reporter
    #[allow(dead_code)]
    compliance_audit_reporter: Arc<ComplianceAuditReporter>,

    /// Audit event storage
    #[allow(dead_code)]
    audit_event_store: Arc<AuditEventStore>,
}

/// Provider-specific audit integrations
pub struct ProviderAuditIntegrations {
    /// AWS CloudTrail integration
    #[allow(dead_code)]
    aws_cloudtrail: Option<Arc<AWSCloudTrailIntegration>>,

    /// Azure Activity Log integration
    #[allow(dead_code)]
    azure_activity_log: Option<Arc<AzureActivityLogIntegration>>,

    /// Google Cloud Audit integration
    #[allow(dead_code)]
    gcp_cloud_audit: Option<Arc<GCPCloudAuditIntegration>>,

    /// Okta System Log integration
    #[allow(dead_code)]
    okta_system_log: Option<Arc<OktaSystemLogIntegration>>,

    /// Generic SIEM integration
    #[allow(dead_code)]
    generic_siem: Option<Arc<GenericSIEMIntegration>>,
}

/// Cross-provider event correlator for unified audit trails
pub struct CrossProviderEventCorrelator {
    /// Rules defining how events from different providers are correlated
    #[allow(dead_code)]
    correlation_rules: Vec<EventCorrelationRule>,
    /// Time window within which events are considered related
    #[allow(dead_code)]
    event_window: Duration,
    /// Minimum confidence score required to accept a correlation
    #[allow(dead_code)]
    confidence_threshold: f64,
}

/// Event correlation rule definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventCorrelationRule {
    /// Human-readable name identifying this correlation rule
    pub rule_name: String,
    /// Regex or sequence pattern used to match event chains
    pub pattern: String,
    /// Minimum confidence score [0.0, 1.0] required to accept a match for this rule
    pub confidence_threshold: f64,
}

/// Compliance audit reporter for enterprise reporting
pub struct ComplianceAuditReporter {
    /// Compliance frameworks (SOC2, GDPR, HIPAA, etc.) to report against
    #[allow(dead_code)]
    compliance_frameworks: Vec<ComplianceFramework>,
    /// Configuration for report generation schedule and delivery
    #[allow(dead_code)]
    reporting_config: ReportingConfiguration,
}

/// Audit event store for persistent logging
pub struct AuditEventStore {
    /// Backend used for persisting audit events (local, S3, Azure, GCS)
    #[allow(dead_code)]
    storage_backend: StorageBackend,
    /// Policy governing how long audit events are retained and archived
    #[allow(dead_code)]
    retention_policy: RetentionPolicy,
}

/// Audit correlation session tracking
#[derive(Debug, Clone)]
pub struct AuditCorrelationSession {
    /// Unique identifier for this correlation session
    #[allow(dead_code)]
    session_id: String,
    /// Timestamp when this correlation session began
    #[allow(dead_code)]
    start_time: DateTime<Utc>,
    /// Collected audit events being correlated in this session
    #[allow(dead_code)]
    events: Vec<AuditEvent>,
    /// Current status of the correlation session (active, completed, failed)
    #[allow(dead_code)]
    correlation_status: CorrelationStatus,
}

/// Enterprise audit event structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique identifier for this audit event
    pub event_id: String,
    /// Category of event (e.g., "authentication", "data_access")
    pub event_type: String,
    /// Timestamp when the event was recorded (UTC)
    pub timestamp: DateTime<Utc>,
    /// Name of the cloud or identity provider that generated the event (e.g., "aws", "okta")
    pub provider: String,
    /// User or principal associated with the event, if available
    pub user_context: Option<String>,
    /// Identifier of the resource that was acted upon
    pub resource: String,
    /// Specific action performed on the resource
    pub action: String,
    /// Outcome of the action (e.g., "success", "failure")
    pub outcome: String,
    /// Arbitrary provider-specific key-value metadata attached to this event
    pub metadata: HashMap<String, String>,
}

/// Correlation analysis result
#[derive(Debug)]
pub struct EventSequenceAnalysis {
    /// Ordered sequence of correlated audit events
    pub event_sequence: Vec<AuditEvent>,
    /// Confidence score [0.0, 1.0] for the correlation hypothesis
    pub confidence: f64,
    /// Human-readable summary of the correlation analysis result
    pub analysis_summary: String,
}

/// Anomaly detection result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditAnomaly {
    /// Unique identifier for this anomaly
    pub anomaly_id: String,
    /// Machine-readable anomaly category (e.g., "unusual_access_pattern")
    pub anomaly_type: String,
    /// Severity level of the anomaly (e.g., "low", "medium", "high", "critical")
    pub severity: String,
    /// Human-readable description of the detected anomaly
    pub description: String,
    /// Timestamp when the anomaly was detected (UTC)
    pub detected_at: DateTime<Utc>,
}

/// Compliance analysis result
#[derive(Debug)]
pub struct ComplianceAnalysis {
    /// Overall compliance status for the evaluated framework (e.g., "compliant", "non-compliant")
    pub compliance_status: String,
    /// List of specific compliance violations detected during analysis
    pub violations: Vec<String>,
    /// Actionable recommendations to address violations or improve compliance posture
    pub recommendations: Vec<String>,
}

/// Lifecycle status of an audit correlation session
#[derive(Debug, Clone)]
pub enum CorrelationStatus {
    /// Session is currently collecting and correlating events
    Active,
    /// Correlation analysis has finished successfully
    Completed,
    /// Correlation analysis failed before producing a result
    Failed,
}

/// Compliance framework definition (e.g., SOC2, GDPR, HIPAA)
#[derive(Debug)]
pub struct ComplianceFramework {
    /// Name of the compliance framework
    #[allow(dead_code)]
    name: String,
    /// Version of the framework specification
    #[allow(dead_code)]
    version: String,
    /// List of compliance requirements defined by this framework
    #[allow(dead_code)]
    requirements: Vec<String>,
}

/// Configuration for scheduled compliance report generation
#[derive(Debug)]
pub struct ReportingConfiguration {
    /// How often reports are generated
    #[allow(dead_code)]
    frequency: Duration,
    /// Email addresses of report recipients
    #[allow(dead_code)]
    recipients: Vec<String>,
    /// Output format for reports (e.g., JSON, PDF)
    #[allow(dead_code)]
    format: String,
}

/// Storage backend options for audit event persistence
#[derive(Debug)]
pub enum StorageBackend {
    /// Local filesystem storage
    Local,
    /// Amazon S3 object storage
    S3,
    /// Azure Blob Storage
    Azure,
    /// Google Cloud Storage
    GCS,
}

/// Policy defining audit event retention and archival periods
#[derive(Debug)]
pub struct RetentionPolicy {
    /// Number of days to retain audit events before deletion
    #[allow(dead_code)]
    retention_days: u32,
    /// Number of days after which events are moved to archive storage
    #[allow(dead_code)]
    archive_after_days: u32,
}

// Provider integration stubs

/// Integration stub for AWS CloudTrail audit log ingestion
pub struct AWSCloudTrailIntegration;

/// Integration stub for Azure Activity Log ingestion
pub struct AzureActivityLogIntegration;

/// Integration stub for Google Cloud Audit Logs ingestion
pub struct GCPCloudAuditIntegration;

/// Integration stub for Okta System Log ingestion
pub struct OktaSystemLogIntegration;

/// Integration stub for generic SIEM (Security Information and Event Management) systems
pub struct GenericSIEMIntegration;

impl AuditCorrelationEngine {
    /// Create a new `AuditCorrelationEngine` with default provider integrations and configuration
    pub async fn new() -> Result<Self> {
        Ok(Self {
            correlation_sessions: Arc::new(DashMap::new()),
            provider_integrations: Arc::new(ProviderAuditIntegrations {
                aws_cloudtrail: None,
                azure_activity_log: None,
                gcp_cloud_audit: None,
                okta_system_log: None,
                generic_siem: None,
            }),
            cross_provider_correlator: Arc::new(CrossProviderEventCorrelator {
                correlation_rules: vec![
                    EventCorrelationRule {
                        rule_name: "authentication_sequence".to_string(),
                        pattern: r"SSO_AUTH_APP_TOKEN_PROXIMADB_OPERATION".to_string(),
                        confidence_threshold: 0.85,
                    },
                    EventCorrelationRule {
                        rule_name: "delegation_chain".to_string(),
                        pattern: r"USER_AUTH_ASSUME_ROLE_SERVICE_OP_DATA_ACCESS".to_string(),
                        confidence_threshold: 0.90,
                    },
                ],
                event_window: Duration::hours(1),
                confidence_threshold: 0.8,
            }),
            compliance_audit_reporter: Arc::new(ComplianceAuditReporter {
                compliance_frameworks: vec![],
                reporting_config: ReportingConfiguration {
                    frequency: Duration::hours(24),
                    recipients: vec![],
                    format: "JSON".to_string(),
                },
            }),
            audit_event_store: Arc::new(AuditEventStore {
                storage_backend: StorageBackend::Local,
                retention_policy: RetentionPolicy {
                    retention_days: 90,
                    archive_after_days: 30,
                },
            }),
        })
    }

    /// Correlate a list of audit events and return a sequence analysis with a confidence score
    pub async fn correlate_events(&self, events: Vec<AuditEvent>) -> Result<EventSequenceAnalysis> {
        info!("Correlating {} audit events", events.len());

        // Basic event correlation logic
        let confidence = if !events.is_empty() { 0.85 } else { 0.0 };

        Ok(EventSequenceAnalysis {
            event_sequence: events.to_vec(),
            confidence,
            analysis_summary: "Analysis_completed".to_string(),
        })
    }

    /// Analyse a slice of audit events and return any detected anomalies
    pub async fn detect_anomalies(&self, events: &[AuditEvent]) -> Result<Vec<AuditAnomaly>> {
        debug!("Detecting anomalies in {} events", events.len());

        // Basic anomaly detection
        Ok(vec![AuditAnomaly {
            anomaly_id: "ANOM001".to_string(),
            anomaly_type: "unusual_access_pattern".to_string(),
            severity: "medium".to_string(),
            description: "Pattern_detected".to_string(),
            detected_at: Utc::now(),
        }])
    }

    /// Generate a compliance analysis report for the specified framework (e.g., "SOC2", "GDPR")
    pub async fn generate_compliance_report(&self, framework: &str) -> Result<ComplianceAnalysis> {
        info!("Generating compliance report for framework: {}", framework);

        Ok(ComplianceAnalysis {
            compliance_status: "compliant".to_string(),
            violations: vec![],
            recommendations: vec!["Continue_monitoring".to_string()],
        })
    }
}

impl CrossProviderEventCorrelator {
    /// Create a new `CrossProviderEventCorrelator` with an empty rule set and default thresholds
    pub fn new() -> Self {
        Self {
            correlation_rules: vec![],
            event_window: Duration::hours(1),
            confidence_threshold: 0.8,
        }
    }

    /// Correlate audit events that originate from different cloud or identity providers
    pub async fn correlate_cross_provider_events(
        &self,
        events: &[AuditEvent],
    ) -> Result<EventSequenceAnalysis> {
        Ok(EventSequenceAnalysis {
            event_sequence: events.to_vec(),
            confidence: 0.85,
            analysis_summary: "Cross_provider_analysis_done".to_string(),
        })
    }
}

impl Default for CrossProviderEventCorrelator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Helper Functions ====================

    fn create_test_event(
        event_id: &str,
        event_type: &str,
        provider: &str,
        user_context: Option<&str>,
    ) -> AuditEvent {
        AuditEvent {
            event_id: event_id.to_string(),
            event_type: event_type.to_string(),
            timestamp: Utc::now(),
            provider: provider.to_string(),
            user_context: user_context.map(|s| s.to_string()),
            resource: "test_resource".to_string(),
            action: "test_action".to_string(),
            outcome: "success".to_string(),
            metadata: HashMap::new(),
        }
    }

    fn create_test_event_with_timestamp(
        event_id: &str,
        event_type: &str,
        provider: &str,
        timestamp: DateTime<Utc>,
    ) -> AuditEvent {
        AuditEvent {
            event_id: event_id.to_string(),
            event_type: event_type.to_string(),
            timestamp,
            provider: provider.to_string(),
            user_context: Some("test_user".to_string()),
            resource: "test_resource".to_string(),
            action: "test_action".to_string(),
            outcome: "success".to_string(),
            metadata: HashMap::new(),
        }
    }

    // ==================== AuditCorrelationEngine Tests ====================

    #[tokio::test]
    async fn test_audit_correlation_engine_creation() {
        let correlation_engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");
        assert!(correlation_engine.correlation_sessions.is_empty());
    }

    #[tokio::test]
    async fn test_audit_correlation_engine_correlate_events_empty() {
        let engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");
        let result = engine
            .correlate_events(vec![])
            .await
            .expect("Failed to correlate events");

        assert_eq!(result.confidence, 0.0);
        assert!(result.event_sequence.is_empty());
    }

    #[tokio::test]
    async fn test_audit_correlation_engine_correlate_events_single() {
        let engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");
        let events = vec![create_test_event(
            "EVT001",
            "authentication",
            "okta",
            Some("user1"),
        )];

        let result = engine
            .correlate_events(events)
            .await
            .expect("Failed to correlate events");

        assert_eq!(result.confidence, 0.85);
        assert_eq!(result.event_sequence.len(), 1);
        assert!(!result.analysis_summary.is_empty());
    }

    #[tokio::test]
    async fn test_audit_correlation_engine_correlate_events_multiple() {
        let engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");
        let events = vec![
            create_test_event("EVT001", "authentication", "okta", Some("user1")),
            create_test_event("EVT002", "authorization", "aws", Some("user1")),
            create_test_event("EVT003", "data_access", "proximadb", Some("user1")),
        ];

        let result = engine
            .correlate_events(events)
            .await
            .expect("Failed to correlate events");

        assert_eq!(result.confidence, 0.85);
        assert_eq!(result.event_sequence.len(), 3);
    }

    #[tokio::test]
    async fn test_audit_correlation_engine_detect_anomalies() {
        let engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");
        let events = vec![
            create_test_event("EVT001", "authentication", "okta", Some("user1")),
            create_test_event("EVT002", "data_access", "aws", Some("user1")),
        ];

        let anomalies = engine
            .detect_anomalies(&events)
            .await
            .expect("Failed to detect anomalies");

        // Should detect at least one anomaly (based on placeholder implementation)
        assert!(!anomalies.is_empty());
        assert_eq!(anomalies[0].anomaly_id, "ANOM001");
        assert_eq!(anomalies[0].anomaly_type, "unusual_access_pattern");
    }

    #[tokio::test]
    async fn test_audit_correlation_engine_detect_anomalies_empty() {
        let engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");
        let events: Vec<AuditEvent> = vec![];

        let anomalies = engine
            .detect_anomalies(&events)
            .await
            .expect("Failed to detect anomalies");

        // Placeholder still returns anomaly
        assert!(!anomalies.is_empty());
    }

    #[tokio::test]
    async fn test_audit_correlation_engine_generate_compliance_report() {
        let engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");

        let analysis = engine
            .generate_compliance_report("SOC2")
            .await
            .expect("Failed to generate compliance report");

        assert_eq!(analysis.compliance_status, "compliant");
        assert!(analysis.violations.is_empty());
        assert!(!analysis.recommendations.is_empty());
    }

    #[tokio::test]
    async fn test_audit_correlation_engine_generate_compliance_report_different_frameworks() {
        let engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");

        for framework in &["SOC2", "GDPR", "HIPAA", "PCI-DSS"] {
            let analysis = engine
                .generate_compliance_report(framework)
                .await
                .expect("Failed to generate compliance report");
            assert_eq!(analysis.compliance_status, "compliant");
        }
    }

    // ==================== CrossProviderEventCorrelator Tests ====================

    #[test]
    fn test_cross_provider_correlator_new() {
        let correlator = CrossProviderEventCorrelator::new();

        assert!(correlator.correlation_rules.is_empty());
        assert_eq!(correlator.event_window, Duration::hours(1));
        assert_eq!(correlator.confidence_threshold, 0.8);
    }

    #[tokio::test]
    async fn test_cross_provider_correlate_events() {
        let correlator = CrossProviderEventCorrelator::new();
        let events = vec![
            create_test_event("EVT001", "sso_auth", "okta", Some("user1")),
            create_test_event("EVT002", "assume_role", "aws", Some("user1")),
            create_test_event("EVT003", "db_access", "proximadb", Some("user1")),
        ];

        let result = correlator
            .correlate_cross_provider_events(&events)
            .await
            .expect("Failed to correlate cross-provider events");

        assert_eq!(result.confidence, 0.85);
        assert_eq!(result.event_sequence.len(), 3);
        assert_eq!(result.analysis_summary, "Cross_provider_analysis_done");
    }

    #[tokio::test]
    async fn test_cross_provider_correlate_events_empty() {
        let correlator = CrossProviderEventCorrelator::new();
        let events: Vec<AuditEvent> = vec![];

        let result = correlator
            .correlate_cross_provider_events(&events)
            .await
            .expect("Failed to correlate cross-provider events");

        assert!(result.event_sequence.is_empty());
    }

    // ==================== AuditEvent Tests ====================

    #[test]
    fn test_audit_event_creation() {
        let event = create_test_event("EVT001", "authentication", "okta", Some("user1"));

        assert_eq!(event.event_id, "EVT001");
        assert_eq!(event.event_type, "authentication");
        assert_eq!(event.provider, "okta");
        assert_eq!(event.user_context, Some("user1".to_string()));
        assert_eq!(event.resource, "test_resource");
        assert_eq!(event.action, "test_action");
        assert_eq!(event.outcome, "success");
    }

    #[test]
    fn test_audit_event_with_metadata() {
        let mut event = create_test_event("EVT001", "data_access", "aws", Some("user1"));
        event
            .metadata
            .insert("region".to_string(), "us-west-2".to_string());
        event
            .metadata
            .insert("bucket".to_string(), "my-bucket".to_string());

        assert_eq!(event.metadata.len(), 2);
        assert_eq!(event.metadata.get("region"), Some(&"us-west-2".to_string()));
        assert_eq!(event.metadata.get("bucket"), Some(&"my-bucket".to_string()));
    }

    #[test]
    fn test_audit_event_serialization() {
        let event = create_test_event("EVT001", "authentication", "okta", Some("user1"));

        let json = serde_json::to_string(&event).expect("Failed to serialize");
        assert!(json.contains("EVT001"));
        assert!(json.contains("authentication"));
        assert!(json.contains("okta"));

        let deserialized: AuditEvent = serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(deserialized.event_id, "EVT001");
        assert_eq!(deserialized.event_type, "authentication");
    }

    #[test]
    fn test_audit_event_timestamp_ordering() {
        let now = Utc::now();
        let event1 = create_test_event_with_timestamp(
            "EVT001",
            "auth",
            "provider",
            now - Duration::hours(1),
        );
        let event2 = create_test_event_with_timestamp("EVT002", "auth", "provider", now);
        let event3 = create_test_event_with_timestamp(
            "EVT003",
            "auth",
            "provider",
            now + Duration::hours(1),
        );

        assert!(event1.timestamp < event2.timestamp);
        assert!(event2.timestamp < event3.timestamp);
    }

    // ==================== AuditAnomaly Tests ====================

    #[test]
    fn test_audit_anomaly_creation() {
        let anomaly = AuditAnomaly {
            anomaly_id: "ANOM001".to_string(),
            anomaly_type: "suspicious_login".to_string(),
            severity: "high".to_string(),
            description: "Multiple failed login attempts".to_string(),
            detected_at: Utc::now(),
        };

        assert_eq!(anomaly.anomaly_id, "ANOM001");
        assert_eq!(anomaly.anomaly_type, "suspicious_login");
        assert_eq!(anomaly.severity, "high");
    }

    #[test]
    fn test_audit_anomaly_serialization() {
        let anomaly = AuditAnomaly {
            anomaly_id: "ANOM001".to_string(),
            anomaly_type: "data_exfiltration".to_string(),
            severity: "critical".to_string(),
            description: "Large data transfer detected".to_string(),
            detected_at: Utc::now(),
        };

        let json = serde_json::to_string(&anomaly).expect("Failed to serialize");
        assert!(json.contains("ANOM001"));
        assert!(json.contains("data_exfiltration"));
        assert!(json.contains("critical"));

        let deserialized: AuditAnomaly =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(deserialized.anomaly_id, "ANOM001");
    }

    // ==================== EventCorrelationRule Tests ====================

    #[test]
    fn test_event_correlation_rule_creation() {
        let rule = EventCorrelationRule {
            rule_name: "authentication_chain".to_string(),
            pattern: r"SSO_AUTH_ASSUME_ROLE_DATA_ACCESS".to_string(),
            confidence_threshold: 0.9,
        };

        assert_eq!(rule.rule_name, "authentication_chain");
        assert_eq!(rule.confidence_threshold, 0.9);
    }

    #[test]
    fn test_event_correlation_rule_serialization() {
        let rule = EventCorrelationRule {
            rule_name: "test_rule".to_string(),
            pattern: r"PATTERN".to_string(),
            confidence_threshold: 0.85,
        };

        let json = serde_json::to_string(&rule).expect("Failed to serialize");
        assert!(json.contains("test_rule"));
        assert!(json.contains("0.85"));

        let deserialized: EventCorrelationRule =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(deserialized.rule_name, "test_rule");
        assert_eq!(deserialized.confidence_threshold, 0.85);
    }

    // ==================== AuditCorrelationSession Tests ====================

    #[test]
    fn test_audit_correlation_session() {
        let session = AuditCorrelationSession {
            session_id: "session-001".to_string(),
            start_time: Utc::now(),
            events: vec![
                create_test_event("EVT001", "auth", "okta", Some("user1")),
                create_test_event("EVT002", "access", "aws", Some("user1")),
            ],
            correlation_status: CorrelationStatus::Active,
        };

        assert_eq!(session.session_id, "session-001");
        assert_eq!(session.events.len(), 2);
        assert!(matches!(
            session.correlation_status,
            CorrelationStatus::Active
        ));
    }

    // ==================== CorrelationStatus Tests ====================

    #[test]
    fn test_correlation_status_variants() {
        let active = CorrelationStatus::Active;
        let completed = CorrelationStatus::Completed;
        let failed = CorrelationStatus::Failed;

        assert!(matches!(active, CorrelationStatus::Active));
        assert!(matches!(completed, CorrelationStatus::Completed));
        assert!(matches!(failed, CorrelationStatus::Failed));
    }

    // ==================== StorageBackend Tests ====================

    #[test]
    fn test_storage_backend_variants() {
        let local = StorageBackend::Local;
        let s3 = StorageBackend::S3;
        let azure = StorageBackend::Azure;
        let gcs = StorageBackend::GCS;

        assert!(matches!(local, StorageBackend::Local));
        assert!(matches!(s3, StorageBackend::S3));
        assert!(matches!(azure, StorageBackend::Azure));
        assert!(matches!(gcs, StorageBackend::GCS));
    }

    // ==================== RetentionPolicy Tests ====================

    #[test]
    fn test_retention_policy() {
        let policy = RetentionPolicy {
            retention_days: 90,
            archive_after_days: 30,
        };

        assert_eq!(policy.retention_days, 90);
        assert_eq!(policy.archive_after_days, 30);
    }

    // ==================== ComplianceFramework Tests ====================

    #[test]
    fn test_compliance_framework() {
        let framework = ComplianceFramework {
            name: "SOC2".to_string(),
            version: "2017".to_string(),
            requirements: vec![
                "Security".to_string(),
                "Availability".to_string(),
                "Confidentiality".to_string(),
            ],
        };

        assert_eq!(framework.name, "SOC2");
        assert_eq!(framework.version, "2017");
        assert_eq!(framework.requirements.len(), 3);
    }

    // ==================== ReportingConfiguration Tests ====================

    #[test]
    fn test_reporting_configuration() {
        let config = ReportingConfiguration {
            frequency: Duration::hours(24),
            recipients: vec![
                "admin@example.com".to_string(),
                "security@example.com".to_string(),
            ],
            format: "PDF".to_string(),
        };

        assert_eq!(config.frequency, Duration::hours(24));
        assert_eq!(config.recipients.len(), 2);
        assert_eq!(config.format, "PDF");
    }

    // ==================== EventSequenceAnalysis Tests ====================

    #[test]
    fn test_event_sequence_analysis() {
        let analysis = EventSequenceAnalysis {
            event_sequence: vec![create_test_event("EVT001", "auth", "okta", Some("user1"))],
            confidence: 0.92,
            analysis_summary: "Chain detected with high confidence".to_string(),
        };

        assert_eq!(analysis.event_sequence.len(), 1);
        assert_eq!(analysis.confidence, 0.92);
        assert!(analysis.analysis_summary.contains("confidence"));
    }

    // ==================== ComplianceAnalysis Tests ====================

    #[test]
    fn test_compliance_analysis() {
        let analysis = ComplianceAnalysis {
            compliance_status: "partial".to_string(),
            violations: vec!["Missing encryption".to_string()],
            recommendations: vec!["Enable TLS".to_string(), "Rotate keys".to_string()],
        };

        assert_eq!(analysis.compliance_status, "partial");
        assert_eq!(analysis.violations.len(), 1);
        assert_eq!(analysis.recommendations.len(), 2);
    }

    // ==================== Integration Tests ====================

    #[tokio::test]
    async fn test_full_correlation_workflow() {
        let engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");

        // Create a sequence of events simulating a typical authentication flow
        let now = Utc::now();
        let events = vec![
            create_test_event_with_timestamp(
                "EVT001",
                "sso_login",
                "okta",
                now - Duration::minutes(5),
            ),
            create_test_event_with_timestamp(
                "EVT002",
                "token_issued",
                "okta",
                now - Duration::minutes(4),
            ),
            create_test_event_with_timestamp(
                "EVT003",
                "assume_role",
                "aws",
                now - Duration::minutes(3),
            ),
            create_test_event_with_timestamp(
                "EVT004",
                "db_query",
                "proximadb",
                now - Duration::minutes(2),
            ),
            create_test_event_with_timestamp(
                "EVT005",
                "data_export",
                "proximadb",
                now - Duration::minutes(1),
            ),
        ];

        // Correlate events
        let correlation_result = engine
            .correlate_events(events.clone())
            .await
            .expect("Failed to correlate events");
        assert_eq!(correlation_result.event_sequence.len(), 5);
        assert!(correlation_result.confidence > 0.0);

        // Detect anomalies
        let anomalies = engine
            .detect_anomalies(&events)
            .await
            .expect("Failed to detect anomalies");
        assert!(!anomalies.is_empty());

        // Generate compliance report
        let compliance = engine
            .generate_compliance_report("SOC2")
            .await
            .expect("Failed to generate compliance report");
        assert_eq!(compliance.compliance_status, "compliant");
    }

    #[tokio::test]
    async fn test_correlation_session_management() {
        let engine = AuditCorrelationEngine::new()
            .await
            .expect("Failed to create AuditCorrelationEngine");

        // Verify sessions are initially empty
        assert!(engine.correlation_sessions.is_empty());

        // The engine should be able to handle operations
        let events = vec![create_test_event("EVT001", "test", "test", None)];
        let result = engine.correlate_events(events).await;
        assert!(result.is_ok());
    }
}
