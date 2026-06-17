//! Tenant-aware entity store implementation - clean and efficient

use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::Arc;
use tracing::debug;

use crate::proto::proximadb_v1::Entity;
use crate::storage::tenant::TenantManager;

/// Enhanced entity store with clean tenant separation
pub struct TenantAwareEntityStore {
    /// Tenant-specific entity storage (tenant_id -> entities)
    tenant_entities: Arc<DashMap<String, Arc<DashMap<String, Entity>>>>,

    /// Tenant-specific entity headers for fast filtering
    tenant_entity_headers: Arc<DashMap<String, Arc<DashMap<String, EntityHeader>>>>,

    /// Entity-to-collection mapping for reverse lookup
    entity_collection_mapping: Arc<DashMap<String, EntityCollectionMapping>>,

    /// Simple audit logger
    audit_logger: Arc<EntityAuditLogger>,

    /// Tenant manager reference
    tenant_manager: Arc<TenantManager>,
}

/// Simple entity header for fast access
#[derive(Debug, Clone)]
pub struct EntityHeader {
    pub entity_id: String,
    pub tenant_id: String,
    pub collection_id: String,
    pub domain_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata_summary: MetadataSummary,
}

/// Metadata summary for fast filtering
#[derive(Debug, Clone)]
pub struct MetadataSummary {
    pub typed_metadata_keys: Vec<String>,
    pub flexible_metadata_keys: Vec<String>,
    pub has_embeddings: bool,
    pub has_relationships: bool,
}

/// Entity-collection mapping
#[derive(Debug, Clone)]
pub struct EntityCollectionMapping {
    pub tenant_id: String,
    pub collection_id: String,
    pub domain_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Backwards-compat alias for [`TenantUserContext`].
pub type UserContext = TenantUserContext;

/// Simple user context for RBAC
#[derive(Debug, Clone)]
pub struct TenantUserContext {
    pub user_id: String,
    pub tenant_id: String,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
}

impl TenantAwareEntityStore {
    /// Create new tenant-aware entity store
    pub fn new(tenant_manager: Arc<TenantManager>) -> Self {
        Self {
            tenant_entities: Arc::new(DashMap::new()),
            tenant_entity_headers: Arc::new(DashMap::new()),
            entity_collection_mapping: Arc::new(DashMap::new()),
            audit_logger: Arc::new(EntityAuditLogger::new()),
            tenant_manager,
        }
    }

    /// Store entity with tenant isolation - clean implementation
    pub async fn store_entity(
        &self,
        tenant_id: &str,
        collection_id: &str,
        entity: Entity,
        user_context: &TenantUserContext,
    ) -> Result<String> {
        // Simple tenant validation
        self.tenant_manager
            .validate_user_tenant_access(&user_context.tenant_id, tenant_id)?;

        // Create entity key with tenant namespace
        let entity_key = format!("{}::{}::{}", tenant_id, collection_id, entity.id);

        // Get or create tenant entity storage
        let tenant_store = self
            .tenant_entities
            .entry(tenant_id.to_string())
            .or_insert_with(|| Arc::new(DashMap::new()));

        // Store entity directly
        tenant_store.insert(entity_key.clone(), entity.clone());

        // Create and store entity header for fast access
        let header = EntityHeader {
            entity_id: entity.id.clone(),
            tenant_id: tenant_id.to_string(),
            collection_id: collection_id.to_string(),
            domain_id: None, // Will be set when domains are linked
            created_at: Utc::now(),
            updated_at: Utc::now(),
            metadata_summary: MetadataSummary::from_entity(&entity),
        };

        let tenant_headers = self
            .tenant_entity_headers
            .entry(tenant_id.to_string())
            .or_insert_with(|| Arc::new(DashMap::new()));

        tenant_headers.insert(entity_key.clone(), header);

        // Store entity-collection mapping
        self.entity_collection_mapping.insert(
            entity_key.clone(),
            EntityCollectionMapping {
                tenant_id: tenant_id.to_string(),
                collection_id: collection_id.to_string(),
                domain_id: None,
                created_at: Utc::now(),
            },
        );

        // Simple audit log
        self.audit_logger
            .log_entity_stored(tenant_id, collection_id, &entity.id, user_context)
            .await?;

        debug!(
            "Stored entity {} in tenant {} collection {}",
            entity.id, tenant_id, collection_id
        );
        Ok(entity_key)
    }

    /// Get entity with tenant validation
    pub async fn get_entity(
        &self,
        tenant_id: &str,
        collection_id: &str,
        entity_id: &str,
        user_context: &TenantUserContext,
    ) -> Result<Option<Entity>> {
        // Validate user can access this tenant
        self.tenant_manager
            .validate_user_tenant_access(&user_context.tenant_id, tenant_id)?;

        let entity_key = format!("{}::{}::{}", tenant_id, collection_id, entity_id);

        // Get tenant entities
        let entity = if let Some(tenant_store) = self.tenant_entities.get(tenant_id) {
            tenant_store.get(&entity_key).map(|e| e.clone())
        } else {
            None
        };

        // Log access if entity found
        if entity.is_some() {
            self.audit_logger
                .log_entity_accessed(tenant_id, collection_id, entity_id, user_context)
                .await?;
        }

        Ok(entity)
    }

    /// List entities in collection with tenant isolation
    pub async fn list_entities_in_collection(
        &self,
        tenant_id: &str,
        collection_id: &str,
        limit: Option<usize>,
        user_context: &TenantUserContext,
    ) -> Result<Vec<Entity>> {
        // Validate tenant access
        self.tenant_manager
            .validate_user_tenant_access(&user_context.tenant_id, tenant_id)?;

        let mut entities = Vec::new();
        let limit = limit.unwrap_or(100);

        if let Some(tenant_store) = self.tenant_entities.get(tenant_id) {
            let collection_prefix = format!("{}::{}::", tenant_id, collection_id);

            for entry in tenant_store.iter() {
                if entry.key().starts_with(&collection_prefix) {
                    entities.push(entry.value().clone());
                    if entities.len() >= limit {
                        break;
                    }
                }
            }
        }

        // Log collection access
        self.audit_logger
            .log_collection_entities_accessed(
                tenant_id,
                collection_id,
                entities.len(),
                user_context,
            )
            .await?;

        Ok(entities)
    }

    /// Delete entity with tenant validation
    pub async fn delete_entity(
        &self,
        tenant_id: &str,
        collection_id: &str,
        entity_id: &str,
        user_context: &TenantUserContext,
    ) -> Result<bool> {
        // Validate tenant access
        self.tenant_manager
            .validate_user_tenant_access(&user_context.tenant_id, tenant_id)?;

        let entity_key = format!("{}::{}::{}", tenant_id, collection_id, entity_id);

        // Remove from tenant store
        let removed = if let Some(tenant_store) = self.tenant_entities.get(tenant_id) {
            tenant_store.remove(&entity_key).is_some()
        } else {
            false
        };

        // Remove header if entity was removed
        if removed {
            if let Some(tenant_headers) = self.tenant_entity_headers.get(tenant_id) {
                tenant_headers.remove(&entity_key);
            }

            // Remove mapping
            self.entity_collection_mapping.remove(&entity_key);
        }

        // Log deletion
        if removed {
            self.audit_logger
                .log_entity_deleted(tenant_id, collection_id, entity_id, user_context)
                .await?;
        }

        Ok(removed)
    }

    /// Get tenant statistics
    pub fn get_tenant_entity_stats(&self, tenant_id: &str) -> TenantEntityStats {
        let entity_count = self
            .tenant_entities
            .get(tenant_id)
            .map_or(0, |store| store.len());

        let header_count = self
            .tenant_entity_headers
            .get(tenant_id)
            .map_or(0, |headers| headers.len());

        TenantEntityStats {
            tenant_id: tenant_id.to_string(),
            total_entities: entity_count,
            total_headers: header_count,
            last_updated: Utc::now(),
        }
    }
}

/// Entity audit logger - simple implementation
pub struct EntityAuditLogger {
    audit_events: Arc<DashMap<String, EntityAuditEvent>>,
}

impl EntityAuditLogger {
    pub fn new() -> Self {
        Self {
            audit_events: Arc::new(DashMap::new()),
        }
    }

    pub async fn log_entity_stored(
        &self,
        tenant_id: &str,
        collection_id: &str,
        entity_id: &str,
        user_context: &TenantUserContext,
    ) -> Result<()> {
        let event = EntityAuditEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: EntityAuditEventType::EntityStored,
            tenant_id: tenant_id.to_string(),
            collection_id: collection_id.to_string(),
            entity_id: entity_id.to_string(),
            user_id: user_context.user_id.clone(),
            timestamp: Utc::now(),
        };

        self.audit_events.insert(event.event_id.clone(), event);
        Ok(())
    }

    pub async fn log_entity_accessed(
        &self,
        tenant_id: &str,
        collection_id: &str,
        entity_id: &str,
        user_context: &TenantUserContext,
    ) -> Result<()> {
        let event = EntityAuditEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: EntityAuditEventType::EntityAccessed,
            tenant_id: tenant_id.to_string(),
            collection_id: collection_id.to_string(),
            entity_id: entity_id.to_string(),
            user_id: user_context.user_id.clone(),
            timestamp: Utc::now(),
        };

        self.audit_events.insert(event.event_id.clone(), event);
        Ok(())
    }

    pub async fn log_entity_deleted(
        &self,
        tenant_id: &str,
        collection_id: &str,
        entity_id: &str,
        user_context: &TenantUserContext,
    ) -> Result<()> {
        let event = EntityAuditEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: EntityAuditEventType::EntityDeleted,
            tenant_id: tenant_id.to_string(),
            collection_id: collection_id.to_string(),
            entity_id: entity_id.to_string(),
            user_id: user_context.user_id.clone(),
            timestamp: Utc::now(),
        };

        self.audit_events.insert(event.event_id.clone(), event);
        Ok(())
    }

    pub async fn log_collection_entities_accessed(
        &self,
        tenant_id: &str,
        collection_id: &str,
        entity_count: usize,
        user_context: &TenantUserContext,
    ) -> Result<()> {
        let event = EntityAuditEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: EntityAuditEventType::CollectionEntitiesAccessed(entity_count),
            tenant_id: tenant_id.to_string(),
            collection_id: collection_id.to_string(),
            entity_id: "N/A".to_string(),
            user_id: user_context.user_id.clone(),
            timestamp: Utc::now(),
        };

        self.audit_events.insert(event.event_id.clone(), event);
        Ok(())
    }
}

impl Default for EntityAuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple audit event
#[derive(Debug, Clone)]
pub struct EntityAuditEvent {
    pub event_id: String,
    pub event_type: EntityAuditEventType,
    pub tenant_id: String,
    pub collection_id: String,
    pub entity_id: String,
    pub user_id: String,
    pub timestamp: DateTime<Utc>,
}

/// Entity audit event types
#[derive(Debug, Clone)]
pub enum EntityAuditEventType {
    EntityStored,
    EntityAccessed,
    EntityDeleted,
    CollectionEntitiesAccessed(usize),
}

/// Tenant entity statistics
#[derive(Debug, Clone)]
pub struct TenantEntityStats {
    pub tenant_id: String,
    pub total_entities: usize,
    pub total_headers: usize,
    pub last_updated: DateTime<Utc>,
}

impl MetadataSummary {
    pub fn from_entity(entity: &Entity) -> Self {
        Self {
            typed_metadata_keys: entity
                .typed_metadata
                .as_ref()
                .map(|tm| tm.fields.keys().cloned().collect())
                .unwrap_or_default(),
            flexible_metadata_keys: entity.flexible_metadata.keys().cloned().collect(),
            has_embeddings: !entity.embeddings.is_empty(),
            has_relationships: false, // Will be enhanced when relationships are implemented
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tenant::context::ResourceLimits;
    use crate::storage::tenant::{ComplianceFramework, Industry, SecurityPolicies, TenantConfig};

    async fn create_test_setup() -> (TenantAwareEntityStore, TenantUserContext) {
        let tenant_manager = Arc::new(TenantManager::new());

        // Create test tenant
        let tenant_config = TenantConfig {
            organization_name: "Test Corp".to_string(),
            industry: Industry::Technology,
            compliance_requirements: vec![ComplianceFramework::SOC2],
            resource_limits: ResourceLimits::default(),
            security_policies: SecurityPolicies::default(),
        };

        tenant_manager
            .create_tenant("test_tenant".to_string(), tenant_config)
            .await
            .expect("failed to create test tenant");

        let entity_store = TenantAwareEntityStore::new(tenant_manager);

        let user_context = TenantUserContext {
            user_id: "test_user".to_string(),
            tenant_id: "test_tenant".to_string(),
            roles: vec!["user".to_string()],
            permissions: vec!["entity_read".to_string(), "entity_write".to_string()],
        };

        (entity_store, user_context)
    }

    #[tokio::test]
    async fn test_entity_storage_and_retrieval() {
        let (entity_store, user_context) = create_test_setup().await;

        let entity = Entity {
            id: "test_entity_1".to_string(),
            typed_metadata: None,
            flexible_metadata: std::collections::HashMap::new(),
            embeddings: vec![],
            relations: vec![],
            ..Default::default()
        };

        // Store entity
        let entity_key = entity_store
            .store_entity(
                "test_tenant",
                "test_collection",
                entity.clone(),
                &user_context,
            )
            .await
            .expect("failed to store entity");

        assert!(entity_key.contains("test_tenant"));
        assert!(entity_key.contains("test_collection"));
        assert!(entity_key.contains("test_entity_1"));

        // Retrieve entity
        let retrieved = entity_store
            .get_entity(
                "test_tenant",
                "test_collection",
                "test_entity_1",
                &user_context,
            )
            .await
            .expect("failed to get entity");

        assert!(retrieved.is_some());
        let entity = retrieved.as_ref().expect("entity should exist");
        assert_eq!(entity.id, "test_entity_1");
    }

    #[tokio::test]
    async fn test_tenant_isolation() {
        let (entity_store, user_context) = create_test_setup().await;

        let entity = Entity {
            id: "isolated_entity".to_string(),
            ..Default::default()
        };

        // Store in tenant A
        entity_store
            .store_entity("test_tenant", "test_collection", entity, &user_context)
            .await
            .expect("failed to store entity");

        // Try to access from different tenant (should fail)
        let wrong_tenant_user = TenantUserContext {
            user_id: "wrong_user".to_string(),
            tenant_id: "different_tenant".to_string(),
            roles: vec!["user".to_string()],
            permissions: vec!["entity_read".to_string()],
        };

        let result = entity_store
            .get_entity(
                "test_tenant", // Requesting tenant A data
                "test_collection",
                "isolated_entity",
                &wrong_tenant_user, // But user is from tenant B
            )
            .await;

        assert!(result.is_err()); // Should be denied
    }

    #[tokio::test]
    async fn test_entity_deletion() {
        let (entity_store, user_context) = create_test_setup().await;

        let entity = Entity {
            id: "deletable_entity".to_string(),
            ..Default::default()
        };

        // Store entity
        entity_store
            .store_entity("test_tenant", "test_collection", entity, &user_context)
            .await
            .expect("failed to store entity");

        // Verify entity exists
        let retrieved = entity_store
            .get_entity(
                "test_tenant",
                "test_collection",
                "deletable_entity",
                &user_context,
            )
            .await
            .expect("failed to get entity");
        assert!(retrieved.is_some());

        // Delete entity
        let deleted = entity_store
            .delete_entity(
                "test_tenant",
                "test_collection",
                "deletable_entity",
                &user_context,
            )
            .await
            .expect("failed to delete entity");
        assert!(deleted);

        // Verify entity is gone
        let retrieved_after = entity_store
            .get_entity(
                "test_tenant",
                "test_collection",
                "deletable_entity",
                &user_context,
            )
            .await
            .expect("failed to get entity");
        assert!(retrieved_after.is_none());
    }

    #[tokio::test]
    async fn test_collection_entity_listing() {
        let (entity_store, user_context) = create_test_setup().await;

        // Store multiple entities
        for i in 1..=5 {
            let entity = Entity {
                id: format!("entity_{}", i),
                ..Default::default()
            };

            entity_store
                .store_entity("test_tenant", "test_collection", entity, &user_context)
                .await
                .expect("failed to store entity");
        }

        // List entities
        let entities = entity_store
            .list_entities_in_collection(
                "test_tenant",
                "test_collection",
                Some(3), // Limit to 3
                &user_context,
            )
            .await
            .expect("failed to list entities");

        assert_eq!(entities.len(), 3); // Should respect limit
    }
}
