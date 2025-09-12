use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

// --- Data Models ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub tenant_id: String,
    pub organization_id: String,
    pub name: String,
    pub status: String, // e.g., "active", "suspended"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organization {
    pub organization_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionOwnership {
    pub collection_id: String,
    pub owner_tenant_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Permission {
    Read,
    Write,
    Admin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionSharing {
    pub collection_id: String,
    pub shared_with_tenant_id: String,
    pub permissions: Vec<Permission>,
}

// --- TenantAccessService Trait ---

#[async_trait]
pub trait TenantAccessService: Send + Sync {
    async fn get_tenant_info(&self, tenant_id: &str) -> Result<Option<Tenant>>;
    async fn check_collection_access(
        &self,
        tenant_id: &str,
        collection_id: &str,
        required_permission: Permission,
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
        permissions: Vec<Permission>,
    ) -> Result<()>;
    async fn revoke_collection_access(
        &self,
        collection_id: &str,
        shared_with_tenant_id: &str,
    ) -> Result<()>;
    async fn get_collection_owner(&self, collection_id: &str) -> Result<Option<String>>;
}

// --- In-Memory Mock Implementation ---

pub struct InMemoryTenantAccessService {
    tenants: RwLock<HashMap<String, Tenant>>,
    organizations: RwLock<HashMap<String, Organization>>,
    collection_ownership: RwLock<HashMap<String, CollectionOwnership>>, // collection_id -> ownership
    collection_sharing: RwLock<HashMap<String, Vec<CollectionSharing>>>, // collection_id -> list of shares
}

impl InMemoryTenantAccessService {
    pub fn new() -> Self {
        let mut tenants = HashMap::new();
        tenants.insert("tenant1".to_string(), Tenant {
            tenant_id: "tenant1".to_string(),
            organization_id: "org1".to_string(),
            name: "Tenant One".to_string(),
            status: "active".to_string(),
        });
        tenants.insert("tenant2".to_string(), Tenant {
            tenant_id: "tenant2".to_string(),
            organization_id: "org1".to_string(),
            name: "Tenant Two".to_string(),
            status: "active".to_string(),
        });
        tenants.insert("tenant3".to_string(), Tenant {
            tenant_id: "tenant3".to_string(),
            organization_id: "org2".to_string(),
            name: "Tenant Three".to_string(),
            status: "active".to_string(),
        });

        let mut organizations = HashMap::new();
        organizations.insert("org1".to_string(), Organization {
            organization_id: "org1".to_string(),
            name: "Organization Alpha".to_string(),
        });
        organizations.insert("org2".to_string(), Organization {
            organization_id: "org2".to_string(),
            name: "Organization Beta".to_string(),
        });

        Self {
            tenants: RwLock::new(tenants),
            organizations: RwLock::new(organizations),
            collection_ownership: RwLock::new(HashMap::new()),
            collection_sharing: RwLock::new(HashMap::new()),
        }
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
        required_permission: Permission,
    ) -> Result<bool> {
        let ownership = self.collection_ownership.read().await;
        let sharing = self.collection_sharing.read().await;

        // 1. Check if tenant is the owner
        if let Some(owner_info) = ownership.get(collection_id) {
            if owner_info.owner_tenant_id == tenant_id {
                info!("Access granted: Tenant {} is owner of collection {}", tenant_id, collection_id);
                return Ok(true); // Owner always has full access
            }
        }

        // 2. Check if collection is shared with the tenant with required permissions
        if let Some(shares) = sharing.get(collection_id) {
            for share in shares {
                if share.shared_with_tenant_id == tenant_id {
                    if share.permissions.contains(&required_permission) {
                        info!("Access granted: Collection {} shared with Tenant {} with {:?} permission", collection_id, tenant_id, required_permission);
                        return Ok(true);
                    } else {
                        warn!("Access denied: Collection {} shared with Tenant {} but missing {:?} permission", collection_id, tenant_id, required_permission);
                    }
                }
            }
        }

        info!("Access denied: Tenant {} has no access to collection {}", tenant_id, collection_id);
        Ok(false)
    }

    async fn get_owned_collections(&self, tenant_id: &str) -> Result<Vec<String>> {
        let ownership = self.collection_ownership.read().await;
        Ok(ownership.iter()
            .filter(|(_, owner_info)| owner_info.owner_tenant_id == tenant_id)
            .map(|(col_id, _)| col_id.clone())
            .collect())
    }

    async fn get_shared_collections(&self, tenant_id: &str) -> Result<Vec<String>> {
        let sharing = self.collection_sharing.read().await;
        let mut shared_cols = Vec::new();
        for (col_id, shares) in sharing.iter() {
            if shares.iter().any(|share| share.shared_with_tenant_id == tenant_id) {
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
        ownership.insert(collection_id.to_string(), CollectionOwnership {
            collection_id: collection_id.to_string(),
            owner_tenant_id: owner_tenant_id.to_string(),
        });
        info!("Recorded ownership: Collection {} owned by Tenant {}", collection_id, owner_tenant_id);
        Ok(())
    }

    async fn grant_collection_access(
        &self,
        collection_id: &str,
        shared_with_tenant_id: &str,
        permissions: Vec<Permission>,
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
        info!("Granted access: Collection {} shared with Tenant {}", collection_id, shared_with_tenant_id);
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
            info!("Revoked access: Collection {} no longer shared with Tenant {}", collection_id, shared_with_tenant_id);
        }
        Ok(())
    }

    async fn get_collection_owner(&self, collection_id: &str) -> Result<Option<String>> {
        let ownership = self.collection_ownership.read().await;
        Ok(ownership.get(collection_id).map(|o| o.owner_tenant_id.clone()))
    }
}
