// NOTE: This tenant access layer is not exported from `services::mod` and has no current
// call sites. Keep implementation changes in sync with future wiring if it becomes active.
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

// --- Data Models ---

#[derive(Debug, Clone)]
pub struct Tenant {
    pub tenant_id: String,
    pub organization_id: String,
    pub name: String,
    pub status: String, // e.g., "active", "suspended"
    pub quotas: ResourceQuotas,
}

#[derive(Debug, Clone)]
pub struct Organization {
    pub organization_id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct CollectionOwnership {
    pub collection_id: String,
    pub owner_tenant_id: String,
}

/// Tenant access permissions for collection-level sharing decisions.
#[derive(Debug, Clone, PartialEq)]
pub enum TenantAccessPermission {
    Read,
    Write,
    Admin,
}

/// Backward-compatible local alias used by existing signatures/tests.
pub type Permission = TenantAccessPermission;

#[derive(Debug, Clone)]
pub struct CollectionSharing {
    pub collection_id: String,
    pub shared_with_tenant_id: String,
    pub permissions: Vec<TenantAccessPermission>,
}

#[derive(Debug, Clone)]
pub struct ResourceQuotas {
    /// Maximum number of collections per tenant
    pub max_collections: Option<u64>,
    /// Maximum total vectors per tenant across all collections
    pub max_total_vectors: Option<u64>,
    /// Maximum storage in bytes
    pub max_storage_bytes: Option<u64>,
    /// Maximum concurrent queries
    pub max_concurrent_queries: Option<u64>,
    /// Maximum API requests per minute
    pub max_requests_per_minute: Option<u64>,
}

impl Default for ResourceQuotas {
    fn default() -> Self {
        Self {
            max_collections: Some(100),
            max_total_vectors: Some(1_000_000),
            max_storage_bytes: Some(10 * 1024 * 1024 * 1024), // 10GB
            max_concurrent_queries: Some(50),
            max_requests_per_minute: Some(1000),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResourceUsage {
    pub tenant_id: String,
    pub collection_count: u64,
    pub total_vector_count: u64,
    pub storage_bytes_used: u64,
    pub concurrent_queries: u64,
    pub requests_this_minute: u64,
}

impl Default for ResourceUsage {
    fn default() -> Self {
        Self {
            tenant_id: String::new(),
            collection_count: 0,
            total_vector_count: 0,
            storage_bytes_used: 0,
            concurrent_queries: 0,
            requests_this_minute: 0,
        }
    }
}

// --- TenantAccessService Trait ---

#[async_trait]
pub trait TenantAccessService: Send + Sync {
    async fn get_tenant_info(&self, tenant_id: &str) -> Result<Option<Tenant>>;
    async fn check_collection_access(
        &self,
        tenant_id: &str,
        collection_id: &str,
        required_permission: TenantAccessPermission,
    ) -> Result<bool>;
    async fn get_owned_collections(&self, tenant_id: &str) -> Result<Vec<String>>;
    async fn get_shared_collections(&self, tenant_id: &str) -> Result<Vec<String>>;
    async fn record_collection_ownership(
        &self,
        collection_id: &str,
        owner_tenant_id: &str,
    ) -> Result<()>;
    async fn grant_collection_access(
        &self,
        collection_id: &str,
        shared_with_tenant_id: &str,
        permissions: Vec<TenantAccessPermission>,
    ) -> Result<()>;
    async fn revoke_collection_access(
        &self,
        collection_id: &str,
        shared_with_tenant_id: &str,
    ) -> Result<()>;
    async fn get_collection_owner(&self, collection_id: &str) -> Result<Option<String>>;

    // Resource quota methods
    async fn check_resource_quota(
        &self,
        tenant_id: &str,
        resource_type: ResourceType,
        requested_amount: u64,
    ) -> Result<bool>;
    async fn get_resource_usage(&self, tenant_id: &str) -> Result<ResourceUsage>;
    async fn update_resource_usage(&self, tenant_id: &str, usage: ResourceUsage) -> Result<()>;
}

#[derive(Debug, Clone)]
pub enum ResourceType {
    Collections,
    Vectors,
    Storage,
    ConcurrentQueries,
    RequestsPerMinute,
}

// --- In-Memory Mock Implementation ---

pub struct InMemoryTenantAccessService {
    tenants: RwLock<HashMap<String, Tenant>>,
    organizations: RwLock<HashMap<String, Organization>>,
    collection_ownership: RwLock<HashMap<String, CollectionOwnership>>, // collection_id -> ownership
    collection_sharing: RwLock<HashMap<String, Vec<CollectionSharing>>>, // collection_id -> list of shares
    resource_usage: RwLock<HashMap<String, ResourceUsage>>, // tenant_id -> current usage
}

impl InMemoryTenantAccessService {
    pub fn new() -> Self {
        let mut tenants = HashMap::new();
        tenants.insert(
            "tenant1".to_string(),
            Tenant {
                tenant_id: "tenant1".to_string(),
                organization_id: "org1".to_string(),
                name: "Tenant One".to_string(),
                status: "active".to_string(),
                quotas: ResourceQuotas::default(),
            },
        );
        tenants.insert(
            "tenant2".to_string(),
            Tenant {
                tenant_id: "tenant2".to_string(),
                organization_id: "org1".to_string(),
                name: "Tenant Two".to_string(),
                status: "active".to_string(),
                quotas: ResourceQuotas::default(),
            },
        );
        tenants.insert(
            "tenant3".to_string(),
            Tenant {
                tenant_id: "tenant3".to_string(),
                organization_id: "org2".to_string(),
                name: "Tenant Three".to_string(),
                status: "active".to_string(),
                quotas: ResourceQuotas::default(),
            },
        );

        let mut organizations = HashMap::new();
        organizations.insert(
            "org1".to_string(),
            Organization {
                organization_id: "org1".to_string(),
                name: "Organization Alpha".to_string(),
            },
        );
        organizations.insert(
            "org2".to_string(),
            Organization {
                organization_id: "org2".to_string(),
                name: "Organization Beta".to_string(),
            },
        );

        Self {
            tenants: RwLock::new(tenants),
            organizations: RwLock::new(organizations),
            collection_ownership: RwLock::new(HashMap::new()),
            collection_sharing: RwLock::new(HashMap::new()),
            resource_usage: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryTenantAccessService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TenantAccessService for InMemoryTenantAccessService {
    async fn get_tenant_info(&self, tenant_id: &str) -> Result<Option<Tenant>> {
        let tenants = self.tenants.read().await;
        Ok(tenants.get(tenant_id).cloned())
    }

    async fn check_collection_access(
        &self,
        tenant_id: &str,
        collection_id: &str,
        required_permission: TenantAccessPermission,
    ) -> Result<bool> {
        let ownership = self.collection_ownership.read().await;
        let sharing = self.collection_sharing.read().await;

        // 1. Check if tenant is the owner
        if let Some(owner_info) = ownership.get(collection_id) {
            if owner_info.owner_tenant_id == tenant_id {
                info!(
                    "Access granted: Tenant {} is owner of collection {}",
                    tenant_id, collection_id
                );
                return Ok(true); // Owner always has full access
            }
        }

        // 2. Check if collection is shared with the tenant with required permissions
        if let Some(shares) = sharing.get(collection_id) {
            for share in shares {
                if share.shared_with_tenant_id == tenant_id {
                    if share.permissions.contains(&required_permission) {
                        info!(
                            "Access granted: Collection {} shared with Tenant {} with {:?} permission",
                            collection_id, tenant_id, required_permission
                        );
                        return Ok(true);
                    } else {
                        warn!(
                            "Access denied: Collection {} shared with Tenant {} but missing {:?} permission",
                            collection_id, tenant_id, required_permission
                        );
                    }
                }
            }
        }

        info!(
            "Access denied: Tenant {} has no access to collection {}",
            tenant_id, collection_id
        );
        Ok(false)
    }

    async fn get_owned_collections(&self, tenant_id: &str) -> Result<Vec<String>> {
        let ownership = self.collection_ownership.read().await;
        Ok(ownership
            .iter()
            .filter(|(_, owner_info)| owner_info.owner_tenant_id == tenant_id)
            .map(|(col_id, _)| col_id.clone())
            .collect())
    }

    async fn get_shared_collections(&self, tenant_id: &str) -> Result<Vec<String>> {
        let sharing = self.collection_sharing.read().await;
        let mut shared_cols = Vec::new();
        for (col_id, shares) in sharing.iter() {
            if shares
                .iter()
                .any(|share| share.shared_with_tenant_id == tenant_id)
            {
                shared_cols.push(col_id.clone());
            }
        }
        Ok(shared_cols)
    }

    async fn record_collection_ownership(
        &self,
        collection_id: &str,
        owner_tenant_id: &str,
    ) -> Result<()> {
        let mut ownership = self.collection_ownership.write().await;
        ownership.insert(
            collection_id.to_string(),
            CollectionOwnership {
                collection_id: collection_id.to_string(),
                owner_tenant_id: owner_tenant_id.to_string(),
            },
        );
        info!(
            "Recorded ownership: Collection {} owned by Tenant {}",
            collection_id, owner_tenant_id
        );
        Ok(())
    }

    async fn grant_collection_access(
        &self,
        collection_id: &str,
        shared_with_tenant_id: &str,
        permissions: Vec<TenantAccessPermission>,
    ) -> Result<()> {
        let mut sharing = self.collection_sharing.write().await;
        let shares_for_col = sharing.entry(collection_id.to_string()).or_default();

        // Remove existing share for this tenant if it exists
        shares_for_col.retain(|s| s.shared_with_tenant_id != shared_with_tenant_id);

        shares_for_col.push(CollectionSharing {
            collection_id: collection_id.to_string(),
            shared_with_tenant_id: shared_with_tenant_id.to_string(),
            permissions,
        });
        info!(
            "Granted access: Collection {} shared with Tenant {}",
            collection_id, shared_with_tenant_id
        );
        Ok(())
    }

    async fn revoke_collection_access(
        &self,
        collection_id: &str,
        shared_with_tenant_id: &str,
    ) -> Result<()> {
        let mut sharing = self.collection_sharing.write().await;
        if let Some(shares_for_col) = sharing.get_mut(collection_id) {
            shares_for_col.retain(|s| s.shared_with_tenant_id != shared_with_tenant_id);
            info!(
                "Revoked access: Collection {} no longer shared with Tenant {}",
                collection_id, shared_with_tenant_id
            );
        }
        Ok(())
    }

    async fn get_collection_owner(&self, collection_id: &str) -> Result<Option<String>> {
        let ownership = self.collection_ownership.read().await;
        Ok(ownership
            .get(collection_id)
            .map(|o| o.owner_tenant_id.clone()))
    }

    async fn check_resource_quota(
        &self,
        tenant_id: &str,
        resource_type: ResourceType,
        requested_amount: u64,
    ) -> Result<bool> {
        let tenants = self.tenants.read().await;
        let usage_map = self.resource_usage.read().await;

        let tenant = match tenants.get(tenant_id) {
            Some(t) => t,
            None => return Ok(false), // Tenant doesn't exist
        };

        let current_usage = usage_map
            .get(tenant_id)
            .cloned()
            .unwrap_or_else(|| ResourceUsage {
                tenant_id: tenant_id.to_string(),
                ..Default::default()
            });

        let quota_limit = match resource_type {
            ResourceType::Collections => tenant.quotas.max_collections,
            ResourceType::Vectors => tenant.quotas.max_total_vectors,
            ResourceType::Storage => tenant.quotas.max_storage_bytes,
            ResourceType::ConcurrentQueries => tenant.quotas.max_concurrent_queries,
            ResourceType::RequestsPerMinute => tenant.quotas.max_requests_per_minute,
        };

        let current_usage_amount = match resource_type {
            ResourceType::Collections => current_usage.collection_count,
            ResourceType::Vectors => current_usage.total_vector_count,
            ResourceType::Storage => current_usage.storage_bytes_used,
            ResourceType::ConcurrentQueries => current_usage.concurrent_queries,
            ResourceType::RequestsPerMinute => current_usage.requests_this_minute,
        };

        // Check if the requested amount would exceed the quota
        match quota_limit {
            Some(limit) => Ok(current_usage_amount + requested_amount <= limit),
            None => Ok(true), // No limit set
        }
    }

    async fn get_resource_usage(&self, tenant_id: &str) -> Result<ResourceUsage> {
        let usage_map = self.resource_usage.read().await;
        Ok(usage_map
            .get(tenant_id)
            .cloned()
            .unwrap_or_else(|| ResourceUsage {
                tenant_id: tenant_id.to_string(),
                ..Default::default()
            }))
    }

    async fn update_resource_usage(&self, tenant_id: &str, usage: ResourceUsage) -> Result<()> {
        let mut usage_map = self.resource_usage.write().await;
        usage_map.insert(tenant_id.to_string(), usage);
        info!(
            "Updated resource usage for tenant {}: {:?}",
            tenant_id,
            usage_map.get(tenant_id)
        );
        Ok(())
    }
}
