//! Tenant management for multi-tenant knowledge intelligence platform
//!
//! Clean, simple tenant isolation without over-engineering.

pub mod context;
pub mod domain;
pub mod entity_store;
pub mod knowledge_graph;
pub mod manager;
pub mod performance;
pub mod rbac;
pub mod resources;

pub use context::{
    BusinessContext, DataSensitivityLevel, PerformanceRequirements, ResourceLimits, TenantConfig,
    TenantContext, TenantStatus,
};
pub use domain::{CollectionDomainMapping, DomainContext, DomainManager};
pub use entity_store::{TenantAwareEntityStore, UserContext};
pub use knowledge_graph::{CollectionDomainBridge, DomainKnowledgeGraph};
pub use manager::TenantManager;
pub use performance::{SLACheckResult, TenantMetrics, TenantPerformanceMonitor, TenantSLA};
pub use rbac::{EnhancedRBACManager, Permission, TenantRole};
pub use resources::TenantResourceTracker;

use serde::{Deserialize, Serialize};

/// Industry classification for tenant business context
///
/// Used to apply industry-specific optimizations, compliance requirements,
/// and business logic for tenant operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Industry {
    /// Financial services industry (banking, insurance, investment)
    Financial,
    /// Healthcare and medical services industry
    Healthcare,
    /// Technology and software industry
    Technology,
    /// Manufacturing and industrial sector
    Manufacturing,
    /// Retail and e-commerce industry
    Retail,
    /// Government and public sector
    Government,
    /// Education and academic institutions
    Education,
    /// Custom or other industry type
    Other(String),
}

/// Compliance frameworks required by tenant
///
/// Indicates which regulatory compliance frameworks the tenant must adhere to,
/// influencing security policies, data handling, and audit requirements.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ComplianceFramework {
    /// SOC 2 (Service Organization Control 2) compliance
    SOC2,
    /// HIPAA (Health Insurance Portability and Accountability Act)
    HIPAA,
    /// GDPR (General Data Protection Regulation)
    GDPR,
    /// Basel III (banking regulatory framework)
    BaselIII,
    /// SOX (Sarbanes-Oxley Act)
    SOX,
    /// ISO 27001 (information security management)
    ISO27001,
    /// FedRAMP (Federal Risk and Authorization Management Program)
    FedRAMP,
    /// Custom or other compliance framework
    Custom(String),
}

/// Security policies for tenant
///
/// Defines authentication, authorization, encryption, and audit requirements
/// for tenant data and operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPolicies {
    /// Require multi-factor authentication for all operations
    pub require_mfa: bool,
    /// Enable encryption for data at rest
    pub encryption_at_rest: bool,
    /// Enable comprehensive audit logging for all operations
    pub audit_all_operations: bool,
    /// Allowed IP ranges for access (empty means no restrictions)
    pub allowed_ip_ranges: Vec<String>,
    /// Session timeout duration in minutes
    pub session_timeout_minutes: u32,
}

impl Default for SecurityPolicies {
    fn default() -> Self {
        Self {
            require_mfa: true,
            encryption_at_rest: true,
            audit_all_operations: true,
            allowed_ip_ranges: vec![],    // No restrictions by default
            session_timeout_minutes: 480, // 8 hours
        }
    }
}

/// Global tenant registry for clean access
static TENANT_REGISTRY: std::sync::OnceLock<TenantManager> = std::sync::OnceLock::new();

/// Initialize global tenant manager
pub fn initialize_tenant_manager() -> &'static TenantManager {
    TENANT_REGISTRY.get_or_init(TenantManager::new)
}

/// Get global tenant manager
pub fn get_tenant_manager() -> Option<&'static TenantManager> {
    TENANT_REGISTRY.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_industry_classification() {
        let financial = Industry::Financial;
        let healthcare = Industry::Healthcare;
        let custom = Industry::Other("Aerospace".to_string());

        assert_ne!(financial, healthcare);
        assert_eq!(custom, Industry::Other("Aerospace".to_string()));
    }

    #[test]
    fn test_compliance_frameworks() {
        let sox = ComplianceFramework::SOX;
        let hipaa = ComplianceFramework::HIPAA;

        assert_ne!(sox, hipaa);
        assert_eq!(sox, ComplianceFramework::SOX);
    }

    #[test]
    fn test_security_policies_default() {
        let policies = SecurityPolicies::default();

        assert!(policies.require_mfa);
        assert!(policies.encryption_at_rest);
        assert!(policies.audit_all_operations);
        assert_eq!(policies.session_timeout_minutes, 480);
    }
}
