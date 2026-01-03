//! Audit Logger Implementation
//!
//! Main audit logging system with comprehensive event tracking,
//! encryption, and compliance reporting capabilities.

use super::storage::AuditStorage;
use super::types::{
    AuditEvent, AuditEventType, AuditResource, AuditResult, SecurityAlert, SecurityAlertSeverity,
    SecurityAlertType,
};
use anyhow::{Result, anyhow};
use chrono::{Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Comprehensive audit logger for enterprise compliance
#[derive(Clone)]
pub struct AuditLogger {
    storage: Arc<dyn AuditStorage + Send + Sync>,
    config: AuditConfig,
    encryption_key: Arc<EncryptionKey>,
    alert_sender: Option<Arc<AlertSender>>,
}

/// Configuration for audit logging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    pub enable_audit_logging: bool,
    pub storage_backend: AuditStorageBackend,
    pub encryption_enabled: bool,
    pub external_audit_endpoint: Option<String>,
    pub retention_days: u32,
    pub enable_real_time_alerts: bool,
    pub alert_webhook_url: Option<String>,
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

/// Audit storage backend options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditStorageBackend {
    File {
        directory: String,
    },
    Database {
        connection_string: String,
    },
    S3 {
        bucket: String,
        region: String,
    },
    Combined {
        primary: Box<AuditStorageBackend>,
        secondary: Box<AuditStorageBackend>,
    },
}

/// Encryption key for sensitive audit data
#[derive(Debug)]
pub struct EncryptionKey {
    key: [u8; 32],
    algorithm: EncryptionAlgorithm,
}

/// Supported encryption algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EncryptionAlgorithm {
    AES256GCM,
    ChaCha20Poly1305,
}

/// Alert sender for real-time security notifications
#[derive(Debug)]
pub struct AlertSender {
    webhook_url: Option<String>,
    email_config: Option<EmailConfig>,
}

/// Email configuration for alerts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub smtp_server: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub from_address: String,
    pub to_addresses: Vec<String>,
}

impl std::fmt::Debug for AuditLogger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLogger")
            .field("config", &self.config)
            .field("encryption_key", &self.encryption_key)
            .field("alert_sender", &self.alert_sender)
            .field("storage", &"<AuditStorage trait object>")
            .finish()
    }
}

impl AuditLogger {
    /// Create new audit logger with configuration
    pub async fn new(config: AuditConfig) -> Result<Self> {
        if !config.enable_audit_logging {
            return Err(anyhow!("Audit logging is disabled in configuration"));
        }

        // Initialize storage backend
        let storage = Self::create_storage_backend(&config).await?;

        // Initialize encryption
        let encryption_key = Arc::new(EncryptionKey::new(config.encryption_enabled)?);

        // Initialize alert sender if configured
        let alert_sender = if config.enable_real_time_alerts {
            Some(Arc::new(AlertSender::new(&config)?))
        } else {
            None
        };

        info!(
            "✅ Audit logger initialized with {} compliance frameworks",
            config.compliance_frameworks.len()
        );

        Ok(Self {
            storage,
            config,
            encryption_key,
            alert_sender,
        })
    }

    /// Log comprehensive audit event
    pub async fn log_event(&self, event: AuditEvent) -> Result<()> {
        debug!(
            "📝 Logging audit event: {:?} for user {:?}",
            event.event_type, event.user_id
        );

        // Step 1: Validate event structure
        self.validate_audit_event(&event)?;

        // Step 2: Enrich event with additional metadata
        let enriched_event = self.enrich_audit_event(event).await?;

        // Step 3: Encrypt sensitive fields if encryption is enabled
        let processed_event = if self.config.encryption_enabled {
            self.encrypt_sensitive_fields(enriched_event)?
        } else {
            enriched_event
        };

        // Step 4: Store in primary audit storage
        self.storage
            .store_audit_event(&processed_event)
            .await
            .map_err(|e| anyhow!("Failed to store audit event: {}", e))?;

        // Step 5: Send to external audit systems if configured
        if let Some(ref external_endpoint) = self.config.external_audit_endpoint
            && let Err(e) = self
                .send_to_external_audit(&processed_event, external_endpoint)
                .await
        {
            warn!("Failed to send audit event to external system: {}", e);
            // Don't fail the operation if external audit fails
        }

        // Step 6: Evaluate for security alerts
        self.evaluate_security_alerts(&processed_event).await?;

        debug!(
            "✅ Audit event logged successfully: {}",
            processed_event.event_id
        );
        Ok(())
    }

    /// Log authentication event
    pub async fn log_authentication_event(
        &self,
        user_id: &str,
        authentication_method: &str,
        result: AuditResult,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Result<()> {
        // Calculate risk score before moving ip_address
        let risk_score = self
            .calculate_authentication_risk(user_id, &ip_address)
            .await;

        let event = AuditEvent {
            event_id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::Authentication,
            user_id: Some(user_id.to_string()),
            resource: AuditResource {
                resource_type: "authentication".to_string(),
                resource_id: "system".to_string(),
                parent_resource: None,
            },
            action: format!("authenticate_{}", authentication_method),
            result,
            details: HashMap::new(),
            ip_address,
            user_agent,
            request_id: None,
            tenant_id: None,
            session_id: None,
            risk_score,
        };

        self.log_event(event).await
    }

    /// Log data access event
    pub async fn log_data_access_event(
        &self,
        user_id: &str,
        tenant_id: Option<String>,
        resource: AuditResource,
        action: &str,
        result: AuditResult,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        let event = AuditEvent {
            event_id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::DataAccess,
            user_id: Some(user_id.to_string()),
            resource,
            action: action.to_string(),
            result,
            details: metadata,
            ip_address: None, // Would be populated by middleware
            user_agent: None, // Would be populated by middleware
            request_id: None, // Would be populated by middleware
            tenant_id,
            session_id: None, // Would be populated by middleware
            risk_score: self.calculate_data_access_risk(user_id, action).await,
        };

        self.log_event(event).await
    }

    /// Create storage backend based on configuration
    async fn create_storage_backend(
        config: &AuditConfig,
    ) -> Result<Arc<dyn AuditStorage + Send + Sync>> {
        match &config.storage_backend {
            AuditStorageBackend::File { directory } => {
                let storage = super::storage::FileAuditStorage::new(directory.clone()).await?;
                Ok(Arc::new(storage))
            }
            AuditStorageBackend::Database { connection_string } => {
                let storage =
                    super::storage::DatabaseAuditStorage::new(connection_string.clone()).await?;
                Ok(Arc::new(storage))
            }
            AuditStorageBackend::S3 { bucket, region } => {
                // Placeholder for S3 storage implementation
                Err(anyhow!("S3 audit storage not yet implemented"))
            }
            AuditStorageBackend::Combined { primary, secondary } => {
                // Placeholder for combined storage implementation
                Err(anyhow!("Combined audit storage not yet implemented"))
            }
        }
    }

    /// Validate audit event structure
    fn validate_audit_event(&self, event: &AuditEvent) -> Result<()> {
        if event.event_id.is_empty() {
            return Err(anyhow!("Event ID is required"));
        }

        if event.action.is_empty() {
            return Err(anyhow!("Action is required"));
        }

        if event.resource.resource_type.is_empty() || event.resource.resource_id.is_empty() {
            return Err(anyhow!("Resource type and ID are required"));
        }

        Ok(())
    }

    /// Enrich audit event with additional metadata
    async fn enrich_audit_event(&self, mut event: AuditEvent) -> Result<AuditEvent> {
        // Add system metadata
        event.details.insert(
            "audit_version".to_string(),
            serde_json::Value::String("1.0".to_string()),
        );
        event.details.insert(
            "system_hostname".to_string(),
            serde_json::Value::String(
                std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string()),
            ),
        );

        // Add compliance framework tags
        event.details.insert(
            "compliance_frameworks".to_string(),
            serde_json::Value::Array(
                self.config
                    .compliance_frameworks
                    .iter()
                    .map(|f| serde_json::Value::String(f.clone()))
                    .collect(),
            ),
        );

        // Add geolocation for IP address if available
        if let Some(ref ip) = event.ip_address
            && let Ok(location) = self.get_ip_geolocation(ip).await
        {
            event
                .details
                .insert("geolocation".to_string(), serde_json::to_value(location)?);
        }

        Ok(event)
    }

    /// Encrypt sensitive fields for privacy compliance
    fn encrypt_sensitive_fields(&self, mut event: AuditEvent) -> Result<AuditEvent> {
        if !self.config.encryption_enabled {
            return Ok(event);
        }

        // Encrypt IP addresses for privacy
        if let Some(ref ip) = event.ip_address {
            let encrypted_ip = self.encryption_key.encrypt(ip.as_bytes())?;
            event.ip_address = Some(crate::utils::encoding::base64_encode(&encrypted_ip));
        }

        // Encrypt user agent strings
        if let Some(ref user_agent) = event.user_agent {
            let encrypted_ua = self.encryption_key.encrypt(user_agent.as_bytes())?;
            event.user_agent = Some(crate::utils::encoding::base64_encode(&encrypted_ua));
        }

        // Encrypt sensitive details
        for (key, value) in &mut event.details {
            if self.is_sensitive_field(key)
                && let serde_json::Value::String(string_value) = value
            {
                let encrypted_value = self.encryption_key.encrypt(string_value.as_bytes())?;
                *value = serde_json::Value::String(crate::utils::encoding::base64_encode(
                    &encrypted_value,
                ));
            }
        }

        Ok(event)
    }

    /// Check if field contains sensitive data
    fn is_sensitive_field(&self, field_name: &str) -> bool {
        let sensitive_fields = [
            "email",
            "phone",
            "ssn",
            "credit_card",
            "password",
            "api_key",
            "token",
            "secret",
            "private_key",
        ];

        sensitive_fields
            .iter()
            .any(|&sensitive| field_name.to_lowercase().contains(sensitive))
    }

    /// Evaluate security alerts from audit events
    async fn evaluate_security_alerts(&self, event: &AuditEvent) -> Result<()> {
        let mut alerts = Vec::new();

        // Check for failed authentication attempts
        if event.event_type == AuditEventType::Authentication
            && let AuditResult::Failure { .. } = &event.result
        {
            let recent_failures = self
                .count_recent_auth_failures(
                    event.user_id.as_deref().unwrap_or("unknown"),
                    Duration::minutes(15),
                )
                .await?;

            if recent_failures > 5 {
                alerts.push(SecurityAlert {
                    alert_id: Uuid::new_v4().to_string(),
                    timestamp: Utc::now(),
                    alert_type: SecurityAlertType::SuspiciousAuthActivity,
                    severity: SecurityAlertSeverity::High,
                    description: format!(
                        "Multiple authentication failures: {} in 15 minutes",
                        recent_failures
                    ),
                    user_id: event.user_id.clone(),
                    ip_address: event.ip_address.clone(),
                    related_event_id: event.event_id.clone(),
                });
            }
        }

        // Check for cross-tenant access attempts
        if let (Some(user_tenant), Some(resource_tenant)) = (&event.tenant_id, &event.tenant_id)
            && user_tenant != resource_tenant
        {
            alerts.push(SecurityAlert {
                alert_id: Uuid::new_v4().to_string(),
                timestamp: Utc::now(),
                alert_type: SecurityAlertType::CrossTenantAccess,
                severity: SecurityAlertSeverity::Critical,
                description: format!(
                    "Cross-tenant access attempt: user tenant {} accessing resource tenant {}",
                    user_tenant, resource_tenant
                ),
                user_id: event.user_id.clone(),
                ip_address: event.ip_address.clone(),
                related_event_id: event.event_id.clone(),
            });
        }

        // Send alerts if any were generated
        for alert in alerts {
            self.send_security_alert(alert).await?;
        }

        Ok(())
    }

    /// Send security alert
    async fn send_security_alert(&self, alert: SecurityAlert) -> Result<()> {
        error!(
            "🚨 SECURITY ALERT: {:?} - {}",
            alert.alert_type, alert.description
        );

        if let Some(ref alert_sender) = self.alert_sender {
            alert_sender.send_alert(&alert).await?;
        }

        // Also log the alert as an audit event
        let alert_event = AuditEvent {
            event_id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type: AuditEventType::SecurityEvent,
            user_id: alert.user_id.clone(),
            resource: AuditResource {
                resource_type: "security_alert".to_string(),
                resource_id: alert.alert_id.clone(),
                parent_resource: None,
            },
            action: format!("security_alert_{:?}", alert.alert_type),
            result: AuditResult::Success,
            details: serde_json::to_value(&alert)?
                .as_object()
                .unwrap()
                .clone()
                .into_iter()
                .collect(),
            ip_address: alert.ip_address,
            user_agent: None,
            request_id: None,
            tenant_id: None,
            session_id: None,
            risk_score: Some(match alert.severity {
                SecurityAlertSeverity::Critical => 0.95,
                SecurityAlertSeverity::High => 0.85,
                SecurityAlertSeverity::Medium => 0.65,
                SecurityAlertSeverity::Low => 0.35,
            }),
        };

        // Store the alert event (but don't create recursive alerts)
        self.storage.store_audit_event(&alert_event).await?;

        Ok(())
    }

    /// Count recent authentication failures for a user
    async fn count_recent_auth_failures(
        &self,
        user_id: &str,
        time_window: Duration,
    ) -> Result<u32> {
        let since = Utc::now() - time_window;

        // Query audit storage for recent auth failures
        let recent_events = self
            .storage
            .query_events(
                Some(AuditEventType::Authentication),
                Some(user_id.to_string()),
                Some(since),
                None,
                Some(100),
            )
            .await?;

        let failure_count = recent_events
            .iter()
            .filter(|event| matches!(event.result, AuditResult::Failure { .. }))
            .count() as u32;

        Ok(failure_count)
    }

    /// Calculate authentication risk score
    async fn calculate_authentication_risk(
        &self,
        user_id: &str,
        ip_address: &Option<String>,
    ) -> Option<f64> {
        let mut risk_score: f64 = 0.0;

        // Check for unusual IP address
        if let Some(ip) = &ip_address
            && self.is_suspicious_ip(ip).await
        {
            risk_score += 0.3;
        }

        // Check recent failure rate
        if let Ok(recent_failures) = self
            .count_recent_auth_failures(user_id, Duration::hours(1))
            .await
        {
            risk_score += (recent_failures as f64) * 0.1;
        }

        // Check time-based patterns (logins at unusual hours)
        let current_hour = Utc::now().hour();
        if current_hour < 6 || current_hour > 22 {
            risk_score += 0.2; // Higher risk for after-hours access
        }

        Some(risk_score.min(1.0))
    }

    /// Calculate data access risk score
    async fn calculate_data_access_risk(&self, user_id: &str, action: &str) -> Option<f64> {
        let mut risk_score: f64 = 0.0;

        // Higher risk for bulk operations
        if action.contains("bulk") || action.contains("export") || action.contains("download") {
            risk_score += 0.4;
        }

        // Higher risk for admin operations
        if action.contains("admin") || action.contains("delete") || action.contains("modify") {
            risk_score += 0.3;
        }

        // Check user's recent activity pattern
        // (Placeholder - real implementation would analyze user behavior)

        Some(risk_score.min(1.0))
    }

    /// Check if IP address is suspicious
    async fn is_suspicious_ip(&self, _ip_address: &str) -> bool {
        // Placeholder for threat intelligence integration
        // Real implementation would check against:
        // - Known malicious IP databases
        // - Geo-location anomalies
        // - VPN/proxy detection
        // - Previous security incidents
        false
    }

    /// Get IP geolocation information
    async fn get_ip_geolocation(&self, _ip_address: &str) -> Result<IpGeolocation> {
        // Placeholder for geolocation service integration
        Ok(IpGeolocation {
            country: "Unknown".to_string(),
            region: "Unknown".to_string(),
            city: "Unknown".to_string(),
            latitude: None,
            longitude: None,
        })
    }

    /// Send to external audit system
    async fn send_to_external_audit(&self, event: &AuditEvent, endpoint: &str) -> Result<()> {
        let client = reqwest::Client::new();

        let response = client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .json(event)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "External audit system returned HTTP {}",
                response.status()
            ));
        }

        debug!("✅ Sent audit event to external system: {}", endpoint);
        Ok(())
    }
}

/// IP geolocation information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpGeolocation {
    pub country: String,
    pub region: String,
    pub city: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl EncryptionKey {
    pub fn new(enabled: bool) -> Result<Self> {
        if !enabled {
            // Return dummy key when encryption is disabled
            return Ok(Self {
                key: [0u8; 32],
                algorithm: EncryptionAlgorithm::AES256GCM,
            });
        }

        // Generate secure random key
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);

        Ok(Self {
            key,
            algorithm: EncryptionAlgorithm::AES256GCM,
        })
    }

    pub fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Placeholder for actual encryption
        // Real implementation would use the configured algorithm
        Ok(data.to_vec())
    }

    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Placeholder for actual decryption
        Ok(data.to_vec())
    }
}

impl AlertSender {
    pub fn new(config: &AuditConfig) -> Result<Self> {
        Ok(Self {
            webhook_url: config.alert_webhook_url.clone(),
            email_config: None, // Would be configured separately
        })
    }

    pub async fn send_alert(&self, alert: &SecurityAlert) -> Result<()> {
        if let Some(ref webhook_url) = self.webhook_url {
            let client = reqwest::Client::new();

            let alert_payload = serde_json::json!({
                "alert_id": alert.alert_id,
                "timestamp": alert.timestamp,
                "type": alert.alert_type,
                "severity": alert.severity,
                "description": alert.description,
                "user_id": alert.user_id,
                "ip_address": alert.ip_address
            });

            let response = client
                .post(webhook_url)
                .header("Content-Type", "application/json")
                .json(&alert_payload)
                .send()
                .await?;

            if response.status().is_success() {
                info!("✅ Security alert sent to webhook: {}", alert.alert_id);
            } else {
                warn!(
                    "⚠️ Failed to send security alert to webhook: HTTP {}",
                    response.status()
                );
            }
        }

        Ok(())
    }
}
