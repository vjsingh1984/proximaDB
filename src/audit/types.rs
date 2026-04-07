//! Audit Types and Data Structures
//!
//! Core types for the comprehensive audit logging system

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Comprehensive audit event structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique identifier for this audit event
    pub event_id: String,
    /// Timestamp when the event occurred (UTC)
    pub timestamp: DateTime<Utc>,
    /// Category of audit event (authentication, data access, etc.)
    pub event_type: AuditEventType,
    /// Identifier of the user who triggered the event, if applicable
    pub user_id: Option<String>,
    /// Resource that was acted upon
    pub resource: AuditResource,
    /// Specific action that was performed on the resource
    pub action: String,
    /// Outcome of the action (success, failure, or partial)
    pub result: AuditResult,
    /// Additional structured metadata associated with this event
    pub details: HashMap<String, serde_json::Value>,
    /// Source IP address of the request, if available
    pub ip_address: Option<String>,
    /// HTTP User-Agent header from the request, if available
    pub user_agent: Option<String>,
    /// Correlation ID of the originating request, if available
    pub request_id: Option<String>,
    /// Tenant context in which the event occurred, if applicable
    pub tenant_id: Option<String>,
    /// Session identifier for grouping related events
    pub session_id: Option<String>,
    /// Computed risk score in the range [0.0, 1.0]; higher means riskier
    pub risk_score: Option<f64>,
}

/// Types of audit events
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuditEventType {
    /// User login, logout, token issuance, or credential verification
    Authentication,
    /// Access-control decisions such as permission checks and role evaluation
    Authorization,
    /// Read operations against stored data (queries, exports, views)
    DataAccess,
    /// Write operations that create, update, or delete stored data
    DataModification,
    /// Changes to system or tenant configuration settings
    SystemConfiguration,
    /// Security-relevant actions such as alerts, policy violations, or anomalies
    SecurityEvent,
    /// Compliance-framework-related events (SOC2, GDPR, HIPAA controls)
    ComplianceEvent,
    /// Performance metrics and SLA-related events
    PerformanceEvent,
    /// Tenant lifecycle operations (create, update, suspend, delete)
    TenantManagement,
    /// External API calls made to or from the system
    APIAccess,
}

/// Resource being audited
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditResource {
    /// Category of the resource (e.g., "collection", "vector", "tenant", "user", "system")
    pub resource_type: String,
    /// Unique identifier of the resource within its type
    pub resource_id: String,
    /// Optional parent resource for hierarchical resource trees (e.g., tenant -> collection -> vector)
    pub parent_resource: Option<Box<AuditResource>>,
}

/// Result of the audited operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditResult {
    /// The operation completed successfully without errors or warnings
    Success,
    /// The operation failed; includes an error code and human-readable message
    Failure {
        /// Machine-readable error code (e.g., "AUTH_FAILED", "PERMISSION_DENIED")
        error_code: String,
        /// Human-readable description of the failure reason
        error_message: String,
    },
    /// The operation completed with non-fatal warnings
    Partial {
        /// List of warning messages describing partial failure conditions
        warnings: Vec<String>,
    },
}

/// Security alert generated from audit analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAlert {
    /// Unique identifier for this security alert
    pub alert_id: String,
    /// Timestamp when the alert was detected (UTC)
    pub timestamp: DateTime<Utc>,
    /// Category of security threat that triggered the alert
    pub alert_type: SecurityAlertType,
    /// Severity level used for prioritizing alert response
    pub severity: SecurityAlertSeverity,
    /// Human-readable explanation of why the alert was generated
    pub description: String,
    /// Identifier of the user associated with the suspicious activity, if known
    pub user_id: Option<String>,
    /// Source IP address associated with the suspicious activity, if known
    pub ip_address: Option<String>,
    /// ID of the audit event that triggered this alert
    pub related_event_id: String,
}

/// Types of security alerts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityAlertType {
    /// Unusual authentication patterns such as repeated failures or logins from unexpected locations
    SuspiciousAuthActivity,
    /// Attempt by a user from one tenant to access resources belonging to another tenant
    CrossTenantAccess,
    /// Attempt to gain higher privileges than those currently assigned
    PrivilegeEscalation,
    /// Suspected large-scale extraction of sensitive data outside normal usage patterns
    DataExfiltration,
    /// API access attempted without valid credentials or from an unauthorized source
    UnauthorizedAPIAccess,
    /// Repeated rapid authentication attempts indicating a brute-force password attack
    BruteForceAttack,
    /// Data access volume or patterns that deviate significantly from the user's baseline
    AnomalousDataAccess,
}

/// Security alert severity levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityAlertSeverity {
    /// Informational; minimal risk, no immediate action required
    Low,
    /// Notable anomaly; should be reviewed during normal operations
    Medium,
    /// Significant threat; requires prompt investigation
    High,
    /// Immediate threat to security or compliance; requires urgent response
    Critical,
}

impl AuditEvent {
    /// Create new audit event
    pub fn new(
        event_type: AuditEventType,
        resource: AuditResource,
        action: String,
        result: AuditResult,
    ) -> Self {
        Self {
            event_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type,
            user_id: None,
            resource,
            action,
            result,
            details: HashMap::new(),
            ip_address: None,
            user_agent: None,
            request_id: None,
            tenant_id: None,
            session_id: None,
            risk_score: None,
        }
    }

    /// Set user context
    pub fn with_user(mut self, user_id: String) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Set tenant context
    pub fn with_tenant(mut self, tenant_id: String) -> Self {
        self.tenant_id = Some(tenant_id);
        self
    }

    /// Set request context
    pub fn with_request_context(
        mut self,
        request_id: String,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        self.request_id = Some(request_id);
        self.ip_address = ip_address;
        self.user_agent = user_agent;
        self
    }

    /// Add detail
    pub fn with_detail(mut self, key: String, value: serde_json::Value) -> Self {
        self.details.insert(key, value);
        self
    }

    /// Set risk score
    pub fn with_risk_score(mut self, risk_score: f64) -> Self {
        self.risk_score = Some(risk_score.clamp(0.0, 1.0));
        self
    }
}

impl AuditResource {
    /// Create new audit resource
    pub fn new(resource_type: String, resource_id: String) -> Self {
        Self {
            resource_type,
            resource_id,
            parent_resource: None,
        }
    }

    /// Set parent resource
    pub fn with_parent(mut self, parent: AuditResource) -> Self {
        self.parent_resource = Some(Box::new(parent));
        self
    }
}

impl SecurityAlert {
    /// Create new security alert
    pub fn new(
        alert_type: SecurityAlertType,
        severity: SecurityAlertSeverity,
        description: String,
    ) -> Self {
        Self {
            alert_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            alert_type,
            severity,
            description,
            user_id: None,
            ip_address: None,
            related_event_id: String::new(),
        }
    }

    /// Set user context
    pub fn with_user(mut self, user_id: String) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Set network context
    pub fn with_network_context(mut self, ip_address: String) -> Self {
        self.ip_address = Some(ip_address);
        self
    }

    /// Set related event
    pub fn with_related_event(mut self, event_id: String) -> Self {
        self.related_event_id = event_id;
        self
    }
}

/// Convert string to AuditEventType
impl std::str::FromStr for AuditEventType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "authentication" => Ok(AuditEventType::Authentication),
            "authorization" => Ok(AuditEventType::Authorization),
            "data_access" => Ok(AuditEventType::DataAccess),
            "data_modification" => Ok(AuditEventType::DataModification),
            "system_configuration" => Ok(AuditEventType::SystemConfiguration),
            "security_event" => Ok(AuditEventType::SecurityEvent),
            "compliance_event" => Ok(AuditEventType::ComplianceEvent),
            "performance_event" => Ok(AuditEventType::PerformanceEvent),
            "tenant_management" => Ok(AuditEventType::TenantManagement),
            "api_access" => Ok(AuditEventType::APIAccess),
            _ => Err(format!("Unknown audit event type: {}", s)),
        }
    }
}

impl std::fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AuditEventType::Authentication => "authentication",
            AuditEventType::Authorization => "authorization",
            AuditEventType::DataAccess => "data_access",
            AuditEventType::DataModification => "data_modification",
            AuditEventType::SystemConfiguration => "system_configuration",
            AuditEventType::SecurityEvent => "security_event",
            AuditEventType::ComplianceEvent => "compliance_event",
            AuditEventType::PerformanceEvent => "performance_event",
            AuditEventType::TenantManagement => "tenant_management",
            AuditEventType::APIAccess => "api_access",
        };
        write!(f, "{}", s)
    }
}

impl std::fmt::Display for SecurityAlertType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SecurityAlertType::SuspiciousAuthActivity => "suspicious_auth_activity",
            SecurityAlertType::CrossTenantAccess => "cross_tenant_access",
            SecurityAlertType::PrivilegeEscalation => "privilege_escalation",
            SecurityAlertType::DataExfiltration => "data_exfiltration",
            SecurityAlertType::UnauthorizedAPIAccess => "unauthorized_api_access",
            SecurityAlertType::BruteForceAttack => "brute_force_attack",
            SecurityAlertType::AnomalousDataAccess => "anomalous_data_access",
        };
        write!(f, "{}", s)
    }
}

impl std::fmt::Display for SecurityAlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SecurityAlertSeverity::Low => "low",
            SecurityAlertSeverity::Medium => "medium",
            SecurityAlertSeverity::High => "high",
            SecurityAlertSeverity::Critical => "critical",
        };
        write!(f, "{}", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== AuditEvent Tests ====================

    #[test]
    fn test_audit_event_new_creates_valid_event() {
        let resource = AuditResource::new("collection".to_string(), "test-collection".to_string());
        let event = AuditEvent::new(
            AuditEventType::DataAccess,
            resource,
            "read".to_string(),
            AuditResult::Success,
        );

        assert!(!event.event_id.is_empty());
        assert_eq!(event.event_type, AuditEventType::DataAccess);
        assert_eq!(event.action, "read");
        assert!(matches!(event.result, AuditResult::Success));
        assert!(event.user_id.is_none());
        assert!(event.tenant_id.is_none());
        assert!(event.risk_score.is_none());
    }

    #[test]
    fn test_audit_event_with_user() {
        let resource = AuditResource::new("collection".to_string(), "test-collection".to_string());
        let event = AuditEvent::new(
            AuditEventType::Authentication,
            resource,
            "login".to_string(),
            AuditResult::Success,
        )
        .with_user("user-123".to_string());

        assert_eq!(event.user_id, Some("user-123".to_string()));
    }

    #[test]
    fn test_audit_event_with_tenant() {
        let resource = AuditResource::new("system".to_string(), "config".to_string());
        let event = AuditEvent::new(
            AuditEventType::SystemConfiguration,
            resource,
            "update".to_string(),
            AuditResult::Success,
        )
        .with_tenant("tenant-abc".to_string());

        assert_eq!(event.tenant_id, Some("tenant-abc".to_string()));
    }

    #[test]
    fn test_audit_event_with_request_context() {
        let resource = AuditResource::new("api".to_string(), "endpoint".to_string());
        let event = AuditEvent::new(
            AuditEventType::APIAccess,
            resource,
            "call".to_string(),
            AuditResult::Success,
        )
        .with_request_context(
            "req-12345".to_string(),
            Some("192.168.1.100".to_string()),
            Some("Mozilla/5.0".to_string()),
        );

        assert_eq!(event.request_id, Some("req-12345".to_string()));
        assert_eq!(event.ip_address, Some("192.168.1.100".to_string()));
        assert_eq!(event.user_agent, Some("Mozilla/5.0".to_string()));
    }

    #[test]
    fn test_audit_event_with_detail() {
        let resource = AuditResource::new("vector".to_string(), "vec-1".to_string());
        let event = AuditEvent::new(
            AuditEventType::DataModification,
            resource,
            "insert".to_string(),
            AuditResult::Success,
        )
        .with_detail("vector_count".to_string(), serde_json::json!(100))
        .with_detail("dimension".to_string(), serde_json::json!(768));

        assert_eq!(event.details.len(), 2);
        assert_eq!(
            event.details.get("vector_count"),
            Some(&serde_json::json!(100))
        );
        assert_eq!(
            event.details.get("dimension"),
            Some(&serde_json::json!(768))
        );
    }

    #[test]
    fn test_audit_event_with_risk_score_clamping() {
        let resource = AuditResource::new("collection".to_string(), "test".to_string());

        // Test clamping above 1.0
        let event_high = AuditEvent::new(
            AuditEventType::SecurityEvent,
            resource.clone(),
            "suspicious".to_string(),
            AuditResult::Success,
        )
        .with_risk_score(1.5);
        assert_eq!(event_high.risk_score, Some(1.0));

        // Test clamping below 0.0
        let event_low = AuditEvent::new(
            AuditEventType::SecurityEvent,
            resource.clone(),
            "normal".to_string(),
            AuditResult::Success,
        )
        .with_risk_score(-0.5);
        assert_eq!(event_low.risk_score, Some(0.0));

        // Test normal value
        let event_normal = AuditEvent::new(
            AuditEventType::SecurityEvent,
            resource,
            "moderate".to_string(),
            AuditResult::Success,
        )
        .with_risk_score(0.65);
        assert_eq!(event_normal.risk_score, Some(0.65));
    }

    #[test]
    fn test_audit_event_builder_chain() {
        let resource = AuditResource::new("collection".to_string(), "products".to_string());
        let event = AuditEvent::new(
            AuditEventType::DataAccess,
            resource,
            "search".to_string(),
            AuditResult::Success,
        )
        .with_user("user-1".to_string())
        .with_tenant("tenant-1".to_string())
        .with_request_context("req-1".to_string(), Some("10.0.0.1".to_string()), None)
        .with_detail("query".to_string(), serde_json::json!("test query"))
        .with_risk_score(0.3);

        assert_eq!(event.user_id, Some("user-1".to_string()));
        assert_eq!(event.tenant_id, Some("tenant-1".to_string()));
        assert_eq!(event.request_id, Some("req-1".to_string()));
        assert_eq!(event.ip_address, Some("10.0.0.1".to_string()));
        assert!(event.user_agent.is_none());
        assert_eq!(event.details.len(), 1);
        assert_eq!(event.risk_score, Some(0.3));
    }

    // ==================== AuditResource Tests ====================

    #[test]
    fn test_audit_resource_new() {
        let resource = AuditResource::new("collection".to_string(), "my-collection".to_string());

        assert_eq!(resource.resource_type, "collection");
        assert_eq!(resource.resource_id, "my-collection");
        assert!(resource.parent_resource.is_none());
    }

    #[test]
    fn test_audit_resource_with_parent() {
        let parent = AuditResource::new("tenant".to_string(), "tenant-123".to_string());
        let child = AuditResource::new("collection".to_string(), "my-collection".to_string())
            .with_parent(parent);

        assert_eq!(child.resource_type, "collection");
        assert_eq!(child.resource_id, "my-collection");
        assert!(child.parent_resource.is_some());

        let parent_ref = child
            .parent_resource
            .as_ref()
            .expect("parent_resource should be Some in with_parent test");
        assert_eq!(parent_ref.resource_type, "tenant");
        assert_eq!(parent_ref.resource_id, "tenant-123");
    }

    #[test]
    fn test_audit_resource_nested_hierarchy() {
        let tenant = AuditResource::new("tenant".to_string(), "tenant-1".to_string());
        let collection = AuditResource::new("collection".to_string(), "collection-1".to_string())
            .with_parent(tenant);
        let vector = AuditResource::new("vector".to_string(), "vector-1".to_string())
            .with_parent(collection);

        assert_eq!(vector.resource_type, "vector");
        let parent = vector
            .parent_resource
            .as_ref()
            .expect("parent_resource should be Some in nested hierarchy test");
        assert_eq!(parent.resource_type, "collection");
        let grandparent = parent
            .parent_resource
            .as_ref()
            .expect("grandparent resource should be Some in nested hierarchy test");
        assert_eq!(grandparent.resource_type, "tenant");
    }

    // ==================== AuditResult Tests ====================

    #[test]
    fn test_audit_result_success() {
        let result = AuditResult::Success;
        assert!(matches!(result, AuditResult::Success));
    }

    #[test]
    fn test_audit_result_failure() {
        let result = AuditResult::Failure {
            error_code: "AUTH_FAILED".to_string(),
            error_message: "Invalid credentials".to_string(),
        };

        if let AuditResult::Failure {
            error_code,
            error_message,
        } = result
        {
            assert_eq!(error_code, "AUTH_FAILED");
            assert_eq!(error_message, "Invalid credentials");
        } else {
            panic!("Expected Failure variant");
        }
    }

    #[test]
    fn test_audit_result_partial() {
        let result = AuditResult::Partial {
            warnings: vec![
                "Some vectors skipped".to_string(),
                "Rate limit applied".to_string(),
            ],
        };

        if let AuditResult::Partial { warnings } = result {
            assert_eq!(warnings.len(), 2);
            assert_eq!(warnings[0], "Some vectors skipped");
        } else {
            panic!("Expected Partial variant");
        }
    }

    // ==================== SecurityAlert Tests ====================

    #[test]
    fn test_security_alert_new() {
        let alert = SecurityAlert::new(
            SecurityAlertType::SuspiciousAuthActivity,
            SecurityAlertSeverity::High,
            "Multiple failed login attempts detected".to_string(),
        );

        assert!(!alert.alert_id.is_empty());
        assert!(matches!(
            alert.alert_type,
            SecurityAlertType::SuspiciousAuthActivity
        ));
        assert!(matches!(alert.severity, SecurityAlertSeverity::High));
        assert_eq!(alert.description, "Multiple failed login attempts detected");
        assert!(alert.user_id.is_none());
        assert!(alert.ip_address.is_none());
        assert!(alert.related_event_id.is_empty());
    }

    #[test]
    fn test_security_alert_with_user() {
        let alert = SecurityAlert::new(
            SecurityAlertType::BruteForceAttack,
            SecurityAlertSeverity::Critical,
            "Brute force attack detected".to_string(),
        )
        .with_user("attacker-user".to_string());

        assert_eq!(alert.user_id, Some("attacker-user".to_string()));
    }

    #[test]
    fn test_security_alert_with_network_context() {
        let alert = SecurityAlert::new(
            SecurityAlertType::CrossTenantAccess,
            SecurityAlertSeverity::Critical,
            "Cross-tenant access attempt".to_string(),
        )
        .with_network_context("192.168.1.50".to_string());

        assert_eq!(alert.ip_address, Some("192.168.1.50".to_string()));
    }

    #[test]
    fn test_security_alert_with_related_event() {
        let alert = SecurityAlert::new(
            SecurityAlertType::PrivilegeEscalation,
            SecurityAlertSeverity::High,
            "Privilege escalation attempt".to_string(),
        )
        .with_related_event("evt-12345".to_string());

        assert_eq!(alert.related_event_id, "evt-12345");
    }

    #[test]
    fn test_security_alert_builder_chain() {
        let alert = SecurityAlert::new(
            SecurityAlertType::DataExfiltration,
            SecurityAlertSeverity::Critical,
            "Large data export detected".to_string(),
        )
        .with_user("suspicious-user".to_string())
        .with_network_context("10.0.0.5".to_string())
        .with_related_event("evt-99999".to_string());

        assert_eq!(alert.user_id, Some("suspicious-user".to_string()));
        assert_eq!(alert.ip_address, Some("10.0.0.5".to_string()));
        assert_eq!(alert.related_event_id, "evt-99999");
    }

    // ==================== AuditEventType Tests ====================

    #[test]
    fn test_audit_event_type_from_str() {
        let parse_result = "authentication"
            .parse::<AuditEventType>()
            .expect("Failed to parse authentication event type");
        assert_eq!(parse_result, AuditEventType::Authentication);

        let parse_result = "authorization"
            .parse::<AuditEventType>()
            .expect("Failed to parse authorization event type");
        assert_eq!(parse_result, AuditEventType::Authorization);

        let parse_result = "data_access"
            .parse::<AuditEventType>()
            .expect("Failed to parse data_access event type");
        assert_eq!(parse_result, AuditEventType::DataAccess);

        let parse_result = "data_modification"
            .parse::<AuditEventType>()
            .expect("Failed to parse data_modification event type");
        assert_eq!(parse_result, AuditEventType::DataModification);

        let parse_result = "system_configuration"
            .parse::<AuditEventType>()
            .expect("Failed to parse system_configuration event type");
        assert_eq!(parse_result, AuditEventType::SystemConfiguration);

        let parse_result = "security_event"
            .parse::<AuditEventType>()
            .expect("Failed to parse security_event event type");
        assert_eq!(parse_result, AuditEventType::SecurityEvent);

        let parse_result = "compliance_event"
            .parse::<AuditEventType>()
            .expect("Failed to parse compliance_event event type");
        assert_eq!(parse_result, AuditEventType::ComplianceEvent);

        let parse_result = "performance_event"
            .parse::<AuditEventType>()
            .expect("Failed to parse performance_event event type");
        assert_eq!(parse_result, AuditEventType::PerformanceEvent);

        let parse_result = "tenant_management"
            .parse::<AuditEventType>()
            .expect("Failed to parse tenant_management event type");
        assert_eq!(parse_result, AuditEventType::TenantManagement);

        let parse_result = "api_access"
            .parse::<AuditEventType>()
            .expect("Failed to parse api_access event type");
        assert_eq!(parse_result, AuditEventType::APIAccess);
    }

    #[test]
    fn test_audit_event_type_from_str_case_insensitive() {
        let parse_result = "AUTHENTICATION"
            .parse::<AuditEventType>()
            .expect("Failed to parse AUTHENTICATION (uppercase)");
        assert_eq!(parse_result, AuditEventType::Authentication);

        let parse_result = "Authentication"
            .parse::<AuditEventType>()
            .expect("Failed to parse Authentication (mixed case)");
        assert_eq!(parse_result, AuditEventType::Authentication);

        let parse_result = "DATA_ACCESS"
            .parse::<AuditEventType>()
            .expect("Failed to parse DATA_ACCESS (uppercase)");
        assert_eq!(parse_result, AuditEventType::DataAccess);
    }

    #[test]
    fn test_audit_event_type_from_str_invalid() {
        let result = "invalid_type".parse::<AuditEventType>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown audit event type"));
    }

    #[test]
    fn test_audit_event_type_display() {
        assert_eq!(AuditEventType::Authentication.to_string(), "authentication");
        assert_eq!(AuditEventType::Authorization.to_string(), "authorization");
        assert_eq!(AuditEventType::DataAccess.to_string(), "data_access");
        assert_eq!(
            AuditEventType::DataModification.to_string(),
            "data_modification"
        );
        assert_eq!(
            AuditEventType::SystemConfiguration.to_string(),
            "system_configuration"
        );
        assert_eq!(AuditEventType::SecurityEvent.to_string(), "security_event");
        assert_eq!(
            AuditEventType::ComplianceEvent.to_string(),
            "compliance_event"
        );
        assert_eq!(
            AuditEventType::PerformanceEvent.to_string(),
            "performance_event"
        );
        assert_eq!(
            AuditEventType::TenantManagement.to_string(),
            "tenant_management"
        );
        assert_eq!(AuditEventType::APIAccess.to_string(), "api_access");
    }

    // ==================== SecurityAlertType Tests ====================

    #[test]
    fn test_security_alert_type_display() {
        assert_eq!(
            SecurityAlertType::SuspiciousAuthActivity.to_string(),
            "suspicious_auth_activity"
        );
        assert_eq!(
            SecurityAlertType::CrossTenantAccess.to_string(),
            "cross_tenant_access"
        );
        assert_eq!(
            SecurityAlertType::PrivilegeEscalation.to_string(),
            "privilege_escalation"
        );
        assert_eq!(
            SecurityAlertType::DataExfiltration.to_string(),
            "data_exfiltration"
        );
        assert_eq!(
            SecurityAlertType::UnauthorizedAPIAccess.to_string(),
            "unauthorized_api_access"
        );
        assert_eq!(
            SecurityAlertType::BruteForceAttack.to_string(),
            "brute_force_attack"
        );
        assert_eq!(
            SecurityAlertType::AnomalousDataAccess.to_string(),
            "anomalous_data_access"
        );
    }

    // ==================== SecurityAlertSeverity Tests ====================

    #[test]
    fn test_security_alert_severity_display() {
        assert_eq!(SecurityAlertSeverity::Low.to_string(), "low");
        assert_eq!(SecurityAlertSeverity::Medium.to_string(), "medium");
        assert_eq!(SecurityAlertSeverity::High.to_string(), "high");
        assert_eq!(SecurityAlertSeverity::Critical.to_string(), "critical");
    }

    // ==================== Serialization Tests ====================

    #[test]
    fn test_audit_event_serialization() {
        let resource = AuditResource::new("collection".to_string(), "test".to_string());
        let event = AuditEvent::new(
            AuditEventType::DataAccess,
            resource,
            "read".to_string(),
            AuditResult::Success,
        )
        .with_user("test-user".to_string());

        let json = serde_json::to_string(&event).expect("Failed to serialize AuditEvent");
        assert!(json.contains("test-user"));
        assert!(json.contains("DataAccess"));

        let deserialized: AuditEvent =
            serde_json::from_str(&json).expect("Failed to deserialize AuditEvent");
        assert_eq!(deserialized.user_id, Some("test-user".to_string()));
        assert_eq!(deserialized.event_type, AuditEventType::DataAccess);
    }

    #[test]
    fn test_audit_result_serialization() {
        let success = AuditResult::Success;
        let success_json = serde_json::to_string(&success).expect("Failed to serialize");
        assert!(success_json.contains("Success"));

        let failure = AuditResult::Failure {
            error_code: "ERR001".to_string(),
            error_message: "Test error".to_string(),
        };
        let failure_json = serde_json::to_string(&failure).expect("Failed to serialize");
        assert!(failure_json.contains("ERR001"));
        assert!(failure_json.contains("Test error"));

        let partial = AuditResult::Partial {
            warnings: vec!["warning1".to_string()],
        };
        let partial_json = serde_json::to_string(&partial).expect("Failed to serialize");
        assert!(partial_json.contains("warning1"));
    }

    #[test]
    fn test_security_alert_serialization() {
        let alert = SecurityAlert::new(
            SecurityAlertType::BruteForceAttack,
            SecurityAlertSeverity::Critical,
            "Test alert".to_string(),
        );

        let json = serde_json::to_string(&alert).expect("Failed to serialize SecurityAlert");
        assert!(json.contains("BruteForceAttack"));
        assert!(json.contains("Critical"));
        assert!(json.contains("Test alert"));

        let deserialized: SecurityAlert =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert!(matches!(
            deserialized.alert_type,
            SecurityAlertType::BruteForceAttack
        ));
        assert!(matches!(
            deserialized.severity,
            SecurityAlertSeverity::Critical
        ));
    }

    #[test]
    fn test_audit_resource_serialization() {
        let parent = AuditResource::new("tenant".to_string(), "t1".to_string());
        let resource =
            AuditResource::new("collection".to_string(), "c1".to_string()).with_parent(parent);

        let json = serde_json::to_string(&resource).expect("Failed to serialize");
        assert!(json.contains("collection"));
        assert!(json.contains("tenant"));

        let deserialized: AuditResource =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(deserialized.resource_type, "collection");
        assert!(deserialized.parent_resource.is_some());
    }

    // ==================== Hash and Eq Tests ====================

    #[test]
    fn test_audit_event_type_hash_eq() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(AuditEventType::Authentication);
        set.insert(AuditEventType::Authorization);
        set.insert(AuditEventType::Authentication); // Duplicate

        assert_eq!(set.len(), 2);
        assert!(set.contains(&AuditEventType::Authentication));
        assert!(set.contains(&AuditEventType::Authorization));
    }

    #[test]
    fn test_audit_event_type_equality() {
        assert_eq!(AuditEventType::DataAccess, AuditEventType::DataAccess);
        assert_ne!(AuditEventType::DataAccess, AuditEventType::DataModification);
    }
}
