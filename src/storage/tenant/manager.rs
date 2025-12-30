//! Tenant manager implementation - clean and simple

use super::resources::TenantResourceUsageSnapshot;
use super::{TenantConfig, TenantContext, TenantResourceTracker, TenantStatus};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::Arc;
use tracing::{info, warn};

/// Clean tenant manager without over-engineering
pub struct TenantManager {
    /// Active tenants
    active_tenants: Arc<DashMap<String, TenantContext>>,

    /// Tenant resource tracking
    tenant_resources: Arc<DashMap<String, TenantResourceTracker>>,

    /// Simple metrics collection
    tenant_metrics: Arc<DashMap<String, TenantMetrics>>,
}

impl TenantManager {
    /// Create new tenant manager
    pub fn new() -> Self {
        Self {
            active_tenants: Arc::new(DashMap::new()),
            tenant_resources: Arc::new(DashMap::new()),
            tenant_metrics: Arc::new(DashMap::new()),
        }
    }

    /// Create new tenant with clean validation
    pub async fn create_tenant(
        &self,
        tenant_id: String,
        config: TenantConfig,
    ) -> Result<TenantContext> {
        // Simple validation
        if tenant_id.is_empty() {
            return Err(anyhow!("Tenant ID cannot be empty"));
        }

        if self.active_tenants.contains_key(&tenant_id) {
            return Err(anyhow!("Tenant {} already exists", tenant_id));
        }

        // Create tenant context directly
        let tenant_context = TenantContext {
            tenant_id: tenant_id.clone(),
            config: config.clone(),
            created_at: chrono::Utc::now(),
            status: TenantStatus::Active,
            domains: Arc::new(DashMap::new()),
            resource_limits: config.resource_limits.clone(),
        };

        // Initialize resource tracker - convert ResourceLimits from context to resources
        let resource_limits = crate::storage::tenant::resources::ResourceLimits {
            max_memory_mb: config.resource_limits.max_memory_mb,
            max_storage_mb: config.resource_limits.max_storage_mb,
            max_operations_per_minute: config.resource_limits.max_operations_per_minute,
            max_concurrent_users: config.resource_limits.max_concurrent_users,
            max_collections: config.resource_limits.max_collections,
            max_domains: config.resource_limits.max_domains,
        };
        let resource_tracker = TenantResourceTracker::new(&tenant_id, &resource_limits);

        // Store tenant
        self.active_tenants
            .insert(tenant_id.clone(), tenant_context.clone());
        self.tenant_resources
            .insert(tenant_id.clone(), resource_tracker);

        // Initialize metrics
        let metrics = TenantMetrics::new(&tenant_id);
        self.tenant_metrics.insert(tenant_id.clone(), metrics);

        info!(
            "Created tenant: {} for organization: {}",
            tenant_id, config.organization_name
        );

        Ok(tenant_context)
    }

    /// Get tenant context with simple validation
    pub fn get_tenant(&self, tenant_id: &str) -> Result<TenantContext> {
        self.active_tenants
            .get(tenant_id)
            .map(|entry| entry.clone())
            .ok_or_else(|| anyhow!("Tenant {} not found", tenant_id))
    }

    /// List all tenants (for admin operations)
    pub fn list_tenants(&self) -> Vec<TenantContext> {
        self.active_tenants
            .iter()
            .map(|entry| entry.clone())
            .collect()
    }

    /// Simple tenant validation for user operations
    pub fn validate_user_tenant_access(
        &self,
        user_tenant_id: &str,
        requested_tenant_id: &str,
    ) -> Result<()> {
        if user_tenant_id != requested_tenant_id {
            warn!(
                "User from tenant {} attempted to access tenant {}",
                user_tenant_id, requested_tenant_id
            );
            return Err(anyhow!(
                "Access denied: user not authorized for tenant {}",
                requested_tenant_id
            ));
        }
        Ok(())
    }

    /// Check if tenant exists and is active
    pub fn is_tenant_active(&self, tenant_id: &str) -> bool {
        self.active_tenants
            .get(tenant_id)
            .map(|entry| entry.status == TenantStatus::Active)
            .unwrap_or(false)
    }

    /// Get tenant resource usage
    pub fn get_tenant_resource_usage(
        &self,
        tenant_id: &str,
    ) -> Option<TenantResourceUsageSnapshot> {
        self.tenant_resources
            .get(tenant_id)
            .map(|tracker| tracker.get_current_usage())
    }
}

/// Simple tenant metrics
#[derive(Debug, Clone)]
pub struct TenantMetrics {
    pub tenant_id: String,
    pub total_operations: u64,
    pub total_entities: u64,
    pub total_collections: u64,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
}

impl TenantMetrics {
    pub fn new(tenant_id: &str) -> Self {
        let now = chrono::Utc::now();
        Self {
            tenant_id: tenant_id.to_string(),
            total_operations: 0,
            total_entities: 0,
            total_collections: 0,
            created_at: now,
            last_activity: now,
        }
    }

    pub fn increment_operations(&mut self) {
        self.total_operations += 1;
        self.last_activity = chrono::Utc::now();
    }

    pub fn increment_entities(&mut self) {
        self.total_entities += 1;
        self.last_activity = chrono::Utc::now();
    }
}

/// Simple resource usage tracking
#[derive(Debug, Clone)]
pub struct TenantResourceUsage {
    pub memory_used_mb: u64,
    pub storage_used_mb: u64,
    pub operations_per_minute: u64,
    pub concurrent_users: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tenant::{ComplianceFramework, Industry, SecurityPolicies};

    #[tokio::test]
    async fn test_tenant_creation() {
        let manager = TenantManager::new();

        let config = TenantConfig {
            organization_name: "Test Corp".to_string(),
            industry: Industry::Technology,
            compliance_requirements: vec![ComplianceFramework::SOC2],
            resource_limits: ResourceLimits::default(),
            security_policies: SecurityPolicies::default(),
        };

        let result = manager
            .create_tenant("test_tenant".to_string(), config)
            .await;
        assert!(result.is_ok());

        let tenant = result.unwrap();
        assert_eq!(tenant.tenant_id, "test_tenant");
        assert_eq!(tenant.config.organization_name, "Test Corp");
    }

    #[tokio::test]
    async fn test_duplicate_tenant_prevention() {
        let manager = TenantManager::new();

        let config = TenantConfig {
            organization_name: "Test Corp".to_string(),
            industry: Industry::Technology,
            compliance_requirements: vec![ComplianceFramework::SOC2],
            resource_limits: ResourceLimits::default(),
            security_policies: SecurityPolicies::default(),
        };

        // First creation should succeed
        let result1 = manager
            .create_tenant("duplicate_test".to_string(), config.clone())
            .await;
        assert!(result1.is_ok());

        // Second creation should fail
        let result2 = manager
            .create_tenant("duplicate_test".to_string(), config)
            .await;
        assert!(result2.is_err());
    }

    #[test]
    fn test_tenant_validation() {
        let manager = TenantManager::new();

        // Should fail for mismatched tenant IDs
        let result = manager.validate_user_tenant_access("tenant_a", "tenant_b");
        assert!(result.is_err());

        // Should succeed for matching tenant IDs
        let result = manager.validate_user_tenant_access("tenant_a", "tenant_a");
        assert!(result.is_ok());
    }
}
