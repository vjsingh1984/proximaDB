//! Advanced Security Features for ProximaDB
//!
//! Implements enhanced security capabilities including:
//! - Multi-Factor Authentication (MFA)
//! - Rate limiting per user/tenant
//! - IP-based access restrictions
//! - Advanced session management
//! - Security monitoring and alerting

use super::unified_rbac::{UnifiedPermission, UnifiedUserContext};
use crate::audit::logger::AuditLogger;

use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::warn;

/// Multi-Factor Authentication (MFA) service
pub struct MFAService {
    /// MFA configuration
    config: MFAConfig,

    /// Active MFA sessions
    active_sessions: Arc<DashMap<String, MFASession>>,

    /// MFA providers (TOTP, SMS, Email, etc.)
    #[allow(dead_code)]
    providers: HashMap<MFAProvider, Box<dyn MFAProviderImpl + Send + Sync>>,

    /// Audit logger for MFA events
    audit_logger: Option<Arc<AuditLogger>>,
}

/// MFA configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MFAConfig {
    pub enabled: bool,
    pub required_for_admin: bool,
    pub required_for_sensitive_data: bool,
    pub allowed_providers: Vec<MFAProvider>,
    pub session_timeout_minutes: u64,
    pub max_attempts: u32,
    pub lockout_duration_minutes: u64,
}

/// MFA provider types
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum MFAProvider {
    #[serde(rename = "totp")]
    TOTP,
    #[serde(rename = "sms")]
    SMS,
    #[serde(rename = "email")]
    Email,
    #[serde(rename = "push")]
    Push,
    #[serde(rename = "hardware_token")]
    HardwareToken,
}

/// MFA session state
#[derive(Debug, Clone)]
pub struct MFASession {
    #[allow(dead_code)]
    pub session_id: String,
    pub user_id: String,
    pub tenant_id: Option<String>,
    pub provider: MFAProvider,
    pub challenge_sent_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub attempts: u32,
    pub verified: bool,
}

/// MFA provider implementation trait
pub trait MFAProviderImpl: Send + Sync {
    fn send_challenge(
        &self,
        user_context: &UnifiedUserContext,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>>;
    fn verify_challenge(
        &self,
        session_id: &str,
        challenge_response: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + '_>>;
    fn is_configured_for_user(
        &self,
        user_context: &UnifiedUserContext,
    ) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + '_>>;
}

impl MFAService {
    /// Create new MFA service
    pub fn new(config: MFAConfig) -> Self {
        Self {
            config,
            active_sessions: Arc::new(DashMap::new()),
            providers: HashMap::new(),
            audit_logger: None,
        }
    }

    /// Set audit logger
    pub fn set_audit_logger(&mut self, audit_logger: Arc<AuditLogger>) {
        self.audit_logger = Some(audit_logger);
    }

    /// Check if MFA is required for user/operation
    pub async fn is_mfa_required(
        &self,
        _user_context: &UnifiedUserContext,
        requested_permission: &UnifiedPermission,
    ) -> bool {
        if !self.config.enabled {
            return false;
        }

        // MFA required for admin operations
        if self.config.required_for_admin {
            match requested_permission {
                UnifiedPermission::SystemAdmin
                | UnifiedPermission::TenantAdmin
                | UnifiedPermission::ConfigureSystem => return true,
                UnifiedPermission::CollectionAdmin(_) | UnifiedPermission::DomainAdmin(_) => {
                    return true;
                }
                _ => {}
            }
        }

        // MFA required for sensitive data access
        if self.config.required_for_sensitive_data {
            match requested_permission {
                UnifiedPermission::RiskDataAccess
                | UnifiedPermission::FinancialDataAccess
                | UnifiedPermission::ComplianceDataAccess => return true,
                _ => {}
            }
        }

        false
    }

    /// Initiate MFA challenge
    pub async fn initiate_mfa_challenge(
        &self,
        user_context: &UnifiedUserContext,
        preferred_provider: Option<MFAProvider>,
    ) -> Result<MFAChallenge> {
        let provider = preferred_provider.unwrap_or_else(|| {
            self.config
                .allowed_providers
                .first()
                .cloned()
                .unwrap_or(MFAProvider::TOTP)
        });

        let session_id = format!("mfa_{}", uuid::Uuid::new_v4());
        let challenge_data = format!("challenge_{}", uuid::Uuid::new_v4());

        // Create MFA session
        let expires_at = Utc::now() + Duration::minutes(self.config.session_timeout_minutes as i64);
        let session = MFASession {
            session_id: session_id.clone(),
            user_id: user_context.user_id.clone(),
            tenant_id: user_context.tenant_id.clone(),
            provider: provider.clone(),
            challenge_sent_at: Utc::now(),
            expires_at,
            attempts: 0,
            verified: false,
        };

        self.active_sessions.insert(session_id.clone(), session);

        // Log MFA challenge initiation
        if let Some(audit_logger) = &self.audit_logger {
            let _ = audit_logger
                .log_event(create_mfa_audit_event(
                    "mfa_challenge_initiated",
                    user_context,
                    Some(&session_id),
                    true,
                ))
                .await;
        }

        Ok(MFAChallenge {
            session_id,
            provider,
            challenge_data,
            expires_at,
        })
    }

    /// Verify MFA challenge response
    pub async fn verify_mfa_challenge(
        &self,
        session_id: &str,
        challenge_response: &str,
    ) -> Result<MFAVerificationResult> {
        let mut session = self
            .active_sessions
            .get_mut(session_id)
            .ok_or_else(|| anyhow!("MFA session not found or expired"))?;

        // Check session expiration
        if Utc::now() > session.expires_at {
            return Err(anyhow!("MFA session expired"));
        }

        // Check attempt limits
        if session.attempts >= self.config.max_attempts {
            return Err(anyhow!("Maximum MFA attempts exceeded"));
        }

        session.attempts += 1;

        // For placeholder implementation, accept "123456" as valid TOTP
        let verification_success = match session.provider {
            MFAProvider::TOTP => challenge_response == "123456",
            MFAProvider::SMS => challenge_response.len() == 6,
            MFAProvider::Email => challenge_response.len() >= 6,
            _ => false,
        };

        if verification_success {
            session.verified = true;

            // Log successful MFA verification
            if let Some(audit_logger) = &self.audit_logger {
                let user_context = UnifiedUserContext {
                    user_id: session.user_id.clone(),
                    tenant_id: session.tenant_id.clone(),
                    roles: vec![],
                    effective_permissions: HashSet::new(),
                    auth_method: super::unified_rbac::AuthMethod::Internal,
                    session_id: session_id.to_string(),
                    expires_at: None,
                    created_at: Utc::now(),
                    metadata: HashMap::new(),
                };

                let _ = audit_logger
                    .log_event(create_mfa_audit_event(
                        "mfa_verification_success",
                        &user_context,
                        Some(session_id),
                        true,
                    ))
                    .await;
            }

            Ok(MFAVerificationResult {
                success: true,
                session_id: session_id.to_string(),
                verified_at: Utc::now(),
                provider: session.provider.clone(),
                error_message: None,
            })
        } else {
            // Log failed MFA verification
            if let Some(audit_logger) = &self.audit_logger {
                let user_context = UnifiedUserContext {
                    user_id: session.user_id.clone(),
                    tenant_id: session.tenant_id.clone(),
                    roles: vec![],
                    effective_permissions: HashSet::new(),
                    auth_method: super::unified_rbac::AuthMethod::Internal,
                    session_id: session_id.to_string(),
                    expires_at: None,
                    created_at: Utc::now(),
                    metadata: HashMap::new(),
                };

                let _ = audit_logger
                    .log_event(create_mfa_audit_event(
                        "mfa_verification_failed",
                        &user_context,
                        Some(session_id),
                        false,
                    ))
                    .await;
            }

            Ok(MFAVerificationResult {
                success: false,
                session_id: session_id.to_string(),
                verified_at: Utc::now(),
                provider: session.provider.clone(),
                error_message: Some("Invalid MFA code".to_string()),
            })
        }
    }

    /// Check if user has completed MFA for session
    pub fn is_mfa_verified(&self, session_id: &str) -> bool {
        self.active_sessions
            .get(session_id)
            .is_some_and(|session| session.verified && Utc::now() <= session.expires_at)
    }

    /// Clean up expired MFA sessions
    pub async fn cleanup_expired_sessions(&self) {
        let now = Utc::now();
        let mut expired_sessions = Vec::new();

        // Find expired sessions
        for entry in self.active_sessions.iter() {
            if now > entry.expires_at {
                expired_sessions.push(entry.session_id.clone());
            }
        }

        // Remove expired sessions
        for session_id in expired_sessions {
            self.active_sessions.remove(&session_id);
        }
    }
}

/// MFA challenge
#[derive(Debug, Clone)]
pub struct MFAChallenge {
    pub session_id: String,
    pub provider: MFAProvider,
    pub challenge_data: String,
    pub expires_at: DateTime<Utc>,
}

/// MFA verification result
#[derive(Debug, Clone)]
pub struct MFAVerificationResult {
    pub success: bool,
    pub session_id: String,
    pub verified_at: DateTime<Utc>,
    pub provider: MFAProvider,
    pub error_message: Option<String>,
}

/// Rate limiting service
pub struct RateLimitingService {
    /// Rate limit configuration
    config: RateLimitConfig,

    /// User rate limit counters
    user_counters: Arc<DashMap<String, RateLimitCounter>>,

    /// Tenant rate limit counters
    tenant_counters: Arc<DashMap<String, RateLimitCounter>>,

    /// IP-based rate limit counters
    ip_counters: Arc<DashMap<String, RateLimitCounter>>,
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub requests_per_minute_per_user: u32,
    pub requests_per_minute_per_tenant: u32,
    pub requests_per_minute_per_ip: u32,
    pub burst_allowance: u32,
    pub cleanup_interval_minutes: u64,
}

/// Rate limit counter
#[derive(Debug, Clone)]
pub struct RateLimitCounter {
    pub requests: u32,
    pub window_start: DateTime<Utc>,
    pub burst_used: u32,
    pub blocked_until: Option<DateTime<Utc>>,
}

impl RateLimitingService {
    /// Create new rate limiting service
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            user_counters: Arc::new(DashMap::new()),
            tenant_counters: Arc::new(DashMap::new()),
            ip_counters: Arc::new(DashMap::new()),
        }
    }

    /// Check if request is allowed within rate limits
    pub async fn check_rate_limit(
        &self,
        user_context: &UnifiedUserContext,
        client_ip: &str,
    ) -> Result<RateLimitResult> {
        if !self.config.enabled {
            return Ok(RateLimitResult::allowed());
        }

        let now = Utc::now();

        // Check user rate limit
        if let Err(e) = self.check_user_rate_limit(&user_context.user_id, now).await {
            return Ok(RateLimitResult::denied("user", e.to_string()));
        }

        // Check tenant rate limit
        if let Some(tenant_id) = &user_context.tenant_id
            && let Err(e) = self.check_tenant_rate_limit(tenant_id, now).await {
                return Ok(RateLimitResult::denied("tenant", e.to_string()));
            }

        // Check IP rate limit
        if let Err(e) = self.check_ip_rate_limit(client_ip, now).await {
            return Ok(RateLimitResult::denied("ip", e.to_string()));
        }

        Ok(RateLimitResult::allowed())
    }

    /// Check user-specific rate limit
    async fn check_user_rate_limit(&self, user_id: &str, now: DateTime<Utc>) -> Result<()> {
        let mut counter = self
            .user_counters
            .entry(user_id.to_string())
            .or_insert_with(|| RateLimitCounter::new(now));

        // Reset counter if window has elapsed
        if now >= counter.window_start + Duration::minutes(1) {
            counter.reset(now);
        }

        // Check rate limit
        if counter.requests >= self.config.requests_per_minute_per_user {
            return Err(anyhow!("User rate limit exceeded"));
        }

        counter.requests += 1;
        Ok(())
    }

    /// Check tenant-specific rate limit
    async fn check_tenant_rate_limit(&self, tenant_id: &str, now: DateTime<Utc>) -> Result<()> {
        let mut counter = self
            .tenant_counters
            .entry(tenant_id.to_string())
            .or_insert_with(|| RateLimitCounter::new(now));

        if now >= counter.window_start + Duration::minutes(1) {
            counter.reset(now);
        }

        if counter.requests >= self.config.requests_per_minute_per_tenant {
            return Err(anyhow!("Tenant rate limit exceeded"));
        }

        counter.requests += 1;
        Ok(())
    }

    /// Check IP-based rate limit
    async fn check_ip_rate_limit(&self, ip: &str, now: DateTime<Utc>) -> Result<()> {
        let mut counter = self
            .ip_counters
            .entry(ip.to_string())
            .or_insert_with(|| RateLimitCounter::new(now));

        if now >= counter.window_start + Duration::minutes(1) {
            counter.reset(now);
        }

        if counter.requests >= self.config.requests_per_minute_per_ip {
            return Err(anyhow!("IP rate limit exceeded"));
        }

        counter.requests += 1;
        Ok(())
    }
}

impl RateLimitCounter {
    fn new(now: DateTime<Utc>) -> Self {
        Self {
            requests: 0,
            window_start: now,
            burst_used: 0,
            blocked_until: None,
        }
    }

    fn reset(&mut self, now: DateTime<Utc>) {
        self.requests = 0;
        self.window_start = now;
        self.burst_used = 0;
        self.blocked_until = None;
    }
}

/// Rate limit check result
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub limit_type: Option<String>,
    pub reason: Option<String>,
    pub retry_after: Option<Duration>,
}

impl RateLimitResult {
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            limit_type: None,
            reason: None,
            retry_after: None,
        }
    }

    pub fn denied(limit_type: &str, reason: String) -> Self {
        Self {
            allowed: false,
            limit_type: Some(limit_type.to_string()),
            reason: Some(reason),
            retry_after: Some(Duration::minutes(1)),
        }
    }
}

/// IP access control service
pub struct IPAccessControlService {
    /// Allowed IP ranges per tenant
    tenant_allowed_ips: Arc<DashMap<String, Vec<IPRange>>>,

    /// Blocked IP addresses
    blocked_ips: Arc<DashMap<String, IPBlock>>,

    /// IP access configuration
    config: IPAccessConfig,
}

/// IP access configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IPAccessConfig {
    pub enabled: bool,
    pub default_allow: bool,
    pub geolocation_blocking: bool,
    pub blocked_countries: Vec<String>,
    pub auto_block_failed_attempts: bool,
    pub auto_block_threshold: u32,
    pub auto_block_duration_minutes: u64,
}

/// IP range for access control
#[derive(Debug, Clone)]
pub struct IPRange {
    pub network: String,
    pub mask: u8,
    pub description: String,
}

/// IP block entry
#[derive(Debug, Clone)]
pub struct IPBlock {
    pub ip: String,
    pub blocked_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub reason: String,
    pub block_count: u32,
}

impl IPAccessControlService {
    /// Create new IP access control service
    pub fn new(config: IPAccessConfig) -> Self {
        Self {
            tenant_allowed_ips: Arc::new(DashMap::new()),
            blocked_ips: Arc::new(DashMap::new()),
            config,
        }
    }

    /// Check if IP address is allowed access
    pub async fn is_ip_allowed(&self, ip: &str, tenant_id: Option<&str>) -> Result<IPAccessResult> {
        if !self.config.enabled {
            return Ok(IPAccessResult::allowed());
        }

        // Check if IP is blocked
        if let Some(block) = self.blocked_ips.get(ip) {
            if let Some(expires_at) = block.expires_at {
                if Utc::now() > expires_at {
                    // Block expired, remove it
                    drop(block);
                    self.blocked_ips.remove(ip);
                } else {
                    return Ok(IPAccessResult::blocked(format!(
                        "IP blocked until {}",
                        expires_at
                    )));
                }
            } else {
                return Ok(IPAccessResult::blocked(
                    "IP permanently blocked".to_string(),
                ));
            }
        }

        // Check tenant-specific IP restrictions
        if let Some(tenant_id) = tenant_id
            && let Some(allowed_ranges) = self.tenant_allowed_ips.get(tenant_id) {
                let ip_allowed = allowed_ranges
                    .iter()
                    .any(|range| self.ip_in_range(ip, range));

                if !ip_allowed {
                    return Ok(IPAccessResult::blocked(
                        "IP not in tenant allowed ranges".to_string(),
                    ));
                }
            }

        Ok(IPAccessResult::allowed())
    }

    /// Block IP address
    pub async fn block_ip(&self, ip: &str, reason: &str, duration: Option<Duration>) {
        let expires_at = duration.map(|d| Utc::now() + d);

        let block = IPBlock {
            ip: ip.to_string(),
            blocked_at: Utc::now(),
            expires_at,
            reason: reason.to_string(),
            block_count: 1,
        };

        self.blocked_ips.insert(ip.to_string(), block);
        warn!("Blocked IP address {}: {}", ip, reason);
    }

    /// Check if IP is in range (simplified implementation)
    fn ip_in_range(&self, _ip: &str, _range: &IPRange) -> bool {
        // Placeholder implementation
        // Real implementation would parse IP addresses and check CIDR ranges
        true
    }
}

/// IP access result
#[derive(Debug, Clone)]
pub struct IPAccessResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

impl IPAccessResult {
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            reason: None,
        }
    }

    pub fn blocked(reason: String) -> Self {
        Self {
            allowed: false,
            reason: Some(reason),
        }
    }
}

/// Create MFA audit event
fn create_mfa_audit_event(
    action: &str,
    user_context: &UnifiedUserContext,
    session_id: Option<&str>,
    success: bool,
) -> crate::audit::types::AuditEvent {
    use crate::audit::types::{AuditEvent, AuditEventType, AuditResource, AuditResult};

    AuditEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        event_type: AuditEventType::Authentication,
        timestamp: Utc::now(),
        user_id: Some(user_context.user_id.clone()),
        tenant_id: user_context.tenant_id.clone(),
        resource: AuditResource {
            resource_type: "mfa".to_string(),
            resource_id: session_id.unwrap_or("unknown").to_string(),
            parent_resource: None,
        },
        action: action.to_string(),
        result: if success {
            AuditResult::Success
        } else {
            AuditResult::Failure {
                error_code: "MFA_FAILED".to_string(),
                error_message: "MFA verification failed".to_string(),
            }
        },
        ip_address: None,
        user_agent: None,
        session_id: session_id.map(|s| s.to_string()),
        request_id: None,
        details: {
            let mut details = HashMap::new();
            details.insert("mfa_action".to_string(), serde_json::json!(action));
            details.insert("success".to_string(), serde_json::json!(success));
            details.insert(
                "user_id".to_string(),
                serde_json::json!(user_context.user_id),
            );
            details
        },
        risk_score: if success { Some(0.0) } else { Some(0.8) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mfa_service_creation() {
        let config = MFAConfig {
            enabled: true,
            required_for_admin: true,
            required_for_sensitive_data: true,
            allowed_providers: vec![MFAProvider::TOTP],
            session_timeout_minutes: 5,
            max_attempts: 3,
            lockout_duration_minutes: 15,
        };

        let mfa_service = MFAService::new(config);
        assert!(mfa_service.config.enabled);
    }

    #[tokio::test]
    async fn test_mfa_requirement_checking() {
        let config = MFAConfig {
            enabled: true,
            required_for_admin: true,
            required_for_sensitive_data: true,
            allowed_providers: vec![MFAProvider::TOTP],
            session_timeout_minutes: 5,
            max_attempts: 3,
            lockout_duration_minutes: 15,
        };

        let mfa_service = MFAService::new(config);
        let user_context = UnifiedUserContext::anonymous();

        // Test admin permission requires MFA
        let admin_permission = UnifiedPermission::SystemAdmin;
        let requires_mfa = mfa_service
            .is_mfa_required(&user_context, &admin_permission)
            .await;
        assert!(requires_mfa);

        // Test sensitive data permission requires MFA
        let sensitive_permission = UnifiedPermission::FinancialDataAccess;
        let requires_mfa_sensitive = mfa_service
            .is_mfa_required(&user_context, &sensitive_permission)
            .await;
        assert!(requires_mfa_sensitive);

        // Test regular permission doesn't require MFA
        let regular_permission = UnifiedPermission::CollectionRead("test".to_string());
        let no_mfa_required = mfa_service
            .is_mfa_required(&user_context, &regular_permission)
            .await;
        assert!(!no_mfa_required);
    }

    #[tokio::test]
    async fn test_rate_limiting_service() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_minute_per_user: 100,
            requests_per_minute_per_tenant: 1000,
            requests_per_minute_per_ip: 200,
            burst_allowance: 10,
            cleanup_interval_minutes: 60,
        };

        let rate_service = RateLimitingService::new(config);
        let user_context = UnifiedUserContext::anonymous();

        // Test rate limit check
        let result = rate_service
            .check_rate_limit(&user_context, "192.168.1.1")
            .await;
        assert!(result.is_ok());

        let rate_result = result.unwrap();
        assert!(rate_result.allowed);
    }

    #[tokio::test]
    async fn test_ip_access_control() {
        let config = IPAccessConfig {
            enabled: true,
            default_allow: true,
            geolocation_blocking: false,
            blocked_countries: vec![],
            auto_block_failed_attempts: true,
            auto_block_threshold: 5,
            auto_block_duration_minutes: 60,
        };

        let ip_service = IPAccessControlService::new(config);

        // Test IP access
        let result = ip_service
            .is_ip_allowed("192.168.1.1", Some("test_tenant"))
            .await;
        assert!(result.is_ok());

        let access_result = result.unwrap();
        assert!(access_result.allowed);

        // Test IP blocking
        ip_service
            .block_ip("192.168.1.100", "Test block", Some(Duration::minutes(60)))
            .await;
        let blocked_result = ip_service
            .is_ip_allowed("192.168.1.100", Some("test_tenant"))
            .await;
        assert!(blocked_result.is_ok());

        let blocked_access = blocked_result.unwrap();
        assert!(!blocked_access.allowed);
    }
}
