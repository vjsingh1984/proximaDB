//! Audit Types and Data Structures
//!
//! Core types for the comprehensive audit logging system

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

/// Comprehensive audit event structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_id: String,
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub user_id: Option<String>,
    pub resource: AuditResource,
    pub action: String,
    pub result: AuditResult,
    pub details: HashMap<String, serde_json::Value>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub request_id: Option<String>,
    pub tenant_id: Option<String>,
    pub session_id: Option<String>,
    pub risk_score: Option<f64>,
}

/// Types of audit events
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditEventType {
    Authentication,
    Authorization,
    DataAccess,
    DataModification,
    SystemConfiguration,
    SecurityEvent,
    ComplianceEvent,
    PerformanceEvent,
    TenantManagement,
    APIAccess,
}

/// Resource being audited
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditResource {
    pub resource_type: String,  // "collection", "vector", "tenant", "user", "system"
    pub resource_id: String,
    pub parent_resource: Option<Box<AuditResource>>,
}

/// Result of the audited operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditResult {
    Success,
    Failure { error_code: String, error_message: String },
    Partial { warnings: Vec<String> },
}

/// Security alert generated from audit analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAlert {
    pub alert_id: String,
    pub timestamp: DateTime<Utc>,
    pub alert_type: SecurityAlertType,
    pub severity: SecurityAlertSeverity,
    pub description: String,
    pub user_id: Option<String>,
    pub ip_address: Option<String>,
    pub related_event_id: String,
}

/// Types of security alerts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityAlertType {
    SuspiciousAuthActivity,
    CrossTenantAccess,
    PrivilegeEscalation,
    DataExfiltration,
    UnauthorizedAPIAccess,
    BruteForceAttack,
    AnomalousDataAccess,
}

/// Security alert severity levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityAlertSeverity {
    Low,
    Medium,
    High,
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
    pub fn with_request_context(mut self, request_id: String, ip_address: Option<String>, user_agent: Option<String>) -> Self {
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
        self.risk_score = Some(risk_score.max(0.0).min(1.0));
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