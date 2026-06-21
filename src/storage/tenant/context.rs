//! Tenant context and configuration - simple data structures

use super::{ComplianceFramework, Industry, SecurityPolicies};
use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Backwards-compat alias for [`StorageTenantContext`].
pub type TenantContext = StorageTenantContext;

/// Simple tenant context
#[derive(Debug, Clone)]
pub struct StorageTenantContext {
    pub tenant_id: String,
    pub config: TenantConfig,
    pub created_at: DateTime<Utc>,
    pub status: TenantStatus,
    pub domains: Arc<DashMap<String, DomainContext>>,
    pub resource_limits: ContextResourceLimits,
}

/// Tenant configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantConfig {
    pub organization_name: String,
    pub industry: Industry,
    pub compliance_requirements: Vec<ComplianceFramework>,
    pub resource_limits: ContextResourceLimits,
    pub security_policies: SecurityPolicies,
}

/// Tenant status
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TenantStatus {
    Active,
    Suspended,
    Inactive,
    Migrating,
}

/// Backwards-compat alias for [`ContextResourceLimits`].
pub type ResourceLimits = ContextResourceLimits;

/// Resource limits for tenant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResourceLimits {
    /// Maximum memory usage in MB
    pub max_memory_mb: u64,

    /// Maximum storage in MB
    pub max_storage_mb: u64,

    /// Maximum operations per minute
    pub max_operations_per_minute: u64,

    /// Maximum concurrent users
    pub max_concurrent_users: u32,

    /// Maximum collections
    pub max_collections: u32,

    /// Maximum domains
    pub max_domains: u32,
}

impl Default for ContextResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_mb: 4096,    // 4GB default
            max_storage_mb: 102400, // 100GB default
            max_operations_per_minute: 10000,
            max_concurrent_users: 100,
            max_collections: 50,
            max_domains: 10,
        }
    }
}

/// Simple domain context within tenant
#[derive(Debug, Clone)]
pub struct DomainContext {
    pub domain_id: String,
    pub tenant_id: String,
    pub domain_name: String,
    pub business_context: BusinessContext,
    pub created_at: DateTime<Utc>,
    pub status: DomainStatus,
}

/// Business context for domain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessContext {
    pub primary_function: String,
    pub data_sensitivity: DataSensitivityLevel,
    pub performance_requirements: PerformanceRequirements,
}

/// Data sensitivity levels for domain classification
///
/// Defines the sensitivity classification for data stored in domains,
/// influencing access controls, encryption requirements, and audit policies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub enum DataSensitivityLevel {
    /// Publicly accessible data with no restrictions
    Public,
    /// Internal business data with organization access
    Internal,
    /// Confidential business data requiring restricted access
    Confidential,
    /// Restricted data with legal or regulatory access controls
    Restricted,
    /// Top secret data with highest security classification
    TopSecret,
}

/// Performance requirements for domain
///
/// Defines service level objectives (SLOs) for latency, throughput,
/// and availability for operations within a domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceRequirements {
    /// Maximum acceptable latency in milliseconds
    pub latency_requirement_ms: u32,
    /// Minimum required throughput in queries per second
    pub throughput_requirement_qps: u32,
    /// Minimum availability requirement (e.g., 0.999 for 99.9%)
    pub availability_requirement: f32,
}

/// Domain status
#[derive(Debug, Clone, PartialEq)]
pub enum DomainStatus {
    Active,
    Inactive,
    Migrating,
}

impl StorageTenantContext {
    /// Build a minimal tenant context carrying just the tenant identity, with
    /// default config/limits. Used by the pgwire write path (TD-064) to scope a
    /// statement to the connection's `database` tenant without loading the full
    /// tenant record — the relational store only needs `tenant_id` to select its
    /// partition, and write authorization is enforced by the catalog-scoped gate.
    pub fn for_tenant_id(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            config: TenantConfig::default(),
            created_at: Utc::now(),
            status: TenantStatus::Active,
            domains: Arc::new(DashMap::new()),
            resource_limits: ContextResourceLimits::default(),
        }
    }

    /// Create or get domain within tenant
    pub async fn get_or_create_domain(
        &self,
        domain_name: &str,
        business_context: BusinessContext,
    ) -> Result<DomainContext> {
        let domain_id = format!("{}::{}", self.tenant_id, domain_name);

        if let Some(domain) = self.domains.get(domain_name) {
            Ok(domain.clone())
        } else {
            let domain_context = DomainContext {
                domain_id,
                tenant_id: self.tenant_id.clone(),
                domain_name: domain_name.to_string(),
                business_context,
                created_at: Utc::now(),
                status: DomainStatus::Active,
            };

            self.domains
                .insert(domain_name.to_string(), domain_context.clone());
            Ok(domain_context)
        }
    }

    /// Get domain by name
    pub fn get_domain(&self, domain_name: &str) -> Option<DomainContext> {
        self.domains.get(domain_name).map(|entry| entry.clone())
    }

    /// List all domains in tenant
    pub fn list_domains(&self) -> Vec<DomainContext> {
        self.domains.iter().map(|entry| entry.clone()).collect()
    }

    /// Check if tenant is active
    pub fn is_active(&self) -> bool {
        self.status == TenantStatus::Active
    }
}

impl Default for TenantConfig {
    fn default() -> Self {
        Self {
            organization_name: "Default Organization".to_string(),
            industry: Industry::Technology,
            compliance_requirements: vec![ComplianceFramework::SOC2],
            resource_limits: ContextResourceLimits::default(),
            security_policies: SecurityPolicies::default(),
        }
    }
}

impl Default for BusinessContext {
    fn default() -> Self {
        Self {
            primary_function: "general".to_string(),
            data_sensitivity: DataSensitivityLevel::Internal,
            performance_requirements: PerformanceRequirements {
                latency_requirement_ms: 100,
                throughput_requirement_qps: 1000,
                availability_requirement: 0.99, // 99%
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tenant::Industry;

    #[test]
    fn test_tenant_context_creation() {
        let config = TenantConfig {
            organization_name: "Test Corp".to_string(),
            industry: Industry::Financial,
            compliance_requirements: vec![ComplianceFramework::SOC2, ComplianceFramework::SOX],
            resource_limits: ContextResourceLimits::default(),
            security_policies: SecurityPolicies::default(),
        };

        let context = StorageTenantContext {
            tenant_id: "test_tenant".to_string(),
            config,
            created_at: Utc::now(),
            status: TenantStatus::Active,
            domains: Arc::new(DashMap::new()),
            resource_limits: ContextResourceLimits::default(),
        };

        assert_eq!(context.tenant_id, "test_tenant");
        assert_eq!(context.config.organization_name, "Test Corp");
        assert_eq!(context.config.industry, Industry::Financial);
        assert!(context.is_active());
    }

    #[tokio::test]
    async fn test_domain_creation() {
        let config = TenantConfig::default();
        let context = StorageTenantContext {
            tenant_id: "test_tenant".to_string(),
            config,
            created_at: Utc::now(),
            status: TenantStatus::Active,
            domains: Arc::new(DashMap::new()),
            resource_limits: ContextResourceLimits::default(),
        };

        let business_context = BusinessContext {
            primary_function: "risk_management".to_string(),
            data_sensitivity: DataSensitivityLevel::Confidential,
            performance_requirements: PerformanceRequirements {
                latency_requirement_ms: 50,
                throughput_requirement_qps: 5000,
                availability_requirement: 0.999,
            },
        };

        let domain = context
            .get_or_create_domain("risk", business_context)
            .await
            .unwrap();
        assert_eq!(domain.domain_name, "risk");
        assert_eq!(domain.tenant_id, "test_tenant");
        assert_eq!(domain.domain_id, "test_tenant::risk");
    }
}
