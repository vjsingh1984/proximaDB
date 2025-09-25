//! Tenant management for multi-tenant knowledge intelligence platform
//!
//! Clean, simple tenant isolation without over-engineering.

pub mod manager;
pub mod context;
pub mod resources;
pub mod entity_store;
pub mod domain;
pub mod rbac;
pub mod knowledge_graph;
pub mod performance;

pub use manager::TenantManager;
pub use context::{TenantContext, TenantConfig, TenantStatus, BusinessContext, DataSensitivityLevel, PerformanceRequirements, ResourceLimits};
pub use resources::{TenantResourceTracker};
pub use entity_store::{TenantAwareEntityStore, UserContext};
pub use domain::{DomainManager, DomainContext, CollectionDomainMapping};
pub use rbac::{EnhancedRBACManager, Permission, TenantRole};
pub use knowledge_graph::{DomainKnowledgeGraph, CollectionDomainBridge};
pub use performance::{TenantPerformanceMonitor, TenantMetrics, TenantSLA, SLACheckResult};

use serde::{Deserialize, Serialize};

/// Industry classification for tenant business context
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Industry {
    Financial,
    Healthcare,
    Technology,
    Manufacturing,
    Retail,
    Government,
    Education,
    Other(String),
}

/// Compliance frameworks required by tenant
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ComplianceFramework {
    SOC2,
    HIPAA,
    GDPR,
    BaselIII,
    SOX,
    ISO27001,
    FedRAMP,
    Custom(String),
}

/// Security policies for tenant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPolicies {
    /// Require MFA for all operations
    pub require_mfa: bool,
    
    /// Encryption at rest required
    pub encryption_at_rest: bool,
    
    /// Audit all operations
    pub audit_all_operations: bool,
    
    /// IP restrictions
    pub allowed_ip_ranges: Vec<String>,
    
    /// Session timeout in minutes
    pub session_timeout_minutes: u32,
}

impl Default for SecurityPolicies {
    fn default() -> Self {
        Self {
            require_mfa: true,
            encryption_at_rest: true,
            audit_all_operations: true,
            allowed_ip_ranges: vec![], // No restrictions by default
            session_timeout_minutes: 480, // 8 hours
        }
    }
}

/// Global tenant registry for clean access
static TENANT_REGISTRY: std::sync::OnceLock<TenantManager> = std::sync::OnceLock::new();

/// Initialize global tenant manager
pub fn initialize_tenant_manager() -> &'static TenantManager {
    TENANT_REGISTRY.get_or_init(|| TenantManager::new())
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