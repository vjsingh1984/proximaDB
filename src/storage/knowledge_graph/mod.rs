//! Multi-Tenant Knowledge Graph Implementation
//!
//! This module provides enterprise-grade knowledge graph capabilities with:
//! - Tenant isolation and security boundaries
//! - Domain-specific entity and relationship stores
//! - RBAC-aware access control at all levels
//! - Cross-domain knowledge composition
//! - Comprehensive audit and provenance tracking

pub mod tenant_graph;
pub mod domain_graph;
pub mod entity_store;
pub mod relationship_store;
pub mod provenance_store;
pub mod collection_bridge;
pub mod rbac;
pub mod types;

// Re-export main types
pub use tenant_graph::TenantKnowledgeGraph;
pub use domain_graph::DomainKnowledgeGraph;
pub use entity_store::DomainEntityStore;
pub use relationship_store::DomainRelationshipStore;
pub use provenance_store::DomainProvenanceStore;
pub use collection_bridge::CollectionEntityBridge;
pub use rbac::{TenantRBACPolicy, DomainRBACFilter, UserContext, SecurityContext};
pub use types::*;

use anyhow::Result;
use std::sync::Arc;

/// Factory for creating tenant-aware knowledge graphs
pub struct KnowledgeGraphFactory;

impl KnowledgeGraphFactory {
    /// Create a new tenant knowledge graph with RBAC policies
    pub async fn create_tenant_graph(
        tenant_id: &str,
        tenant_config: TenantConfig,
    ) -> Result<Arc<TenantKnowledgeGraph>> {
        let knowledge_graph = TenantKnowledgeGraph::new(
            tenant_id.to_string(),
            tenant_config,
        ).await?;
        
        Ok(Arc::new(knowledge_graph))
    }
    
    /// Create domain-specific knowledge graph within a tenant
    pub async fn create_domain_graph(
        tenant_id: &str,
        domain_name: &str,
        domain_config: DomainConfig,
        rbac_policy: Arc<TenantRBACPolicy>,
    ) -> Result<Arc<DomainKnowledgeGraph>> {
        let domain_id = format!("{}::{}", tenant_id, domain_name);
        
        let domain_graph = DomainKnowledgeGraph::new(
            domain_id,
            domain_config,
            rbac_policy,
        ).await?;
        
        Ok(Arc::new(domain_graph))
    }
    
    /// Migrate from global entity store to tenant-aware architecture
    pub async fn migrate_from_global_store(
        global_store: &crate::storage::entity_store::ProximaEntityStore,
        migration_plan: GlobalToTenantMigrationPlan,
    ) -> Result<Vec<Arc<TenantKnowledgeGraph>>> {
        let mut tenant_graphs = Vec::new();
        
        for tenant_migration in migration_plan.tenant_migrations {
            let tenant_graph = Self::create_tenant_graph(
                &tenant_migration.tenant_id,
                tenant_migration.tenant_config,
            ).await?;
            
            // Migrate entities and relationships
            for domain_migration in tenant_migration.domain_migrations {
                let domain = tenant_graph.get_or_create_domain(&domain_migration.domain_name).await?;
                
                // Migrate entities
                for entity_migration in domain_migration.entity_migrations {
                    if let Some(entity) = global_store.get_entity_by_id(&entity_migration.entity_id).await? {
                        domain.entity_store.migrate_entity(entity, entity_migration.permissions).await?;
                    }
                }
                
                // Migrate relationships
                for relationship_migration in domain_migration.relationship_migrations {
                    domain.relationship_store.migrate_relationship(
                        relationship_migration.relationship,
                        relationship_migration.permissions,
                    ).await?;
                }
            }
            
            tenant_graphs.push(tenant_graph);
        }
        
        Ok(tenant_graphs)
    }
}

/// Global registry for tenant knowledge graphs
pub struct TenantKnowledgeGraphRegistry {
    tenant_graphs: Arc<dashmap::DashMap<String, Arc<TenantKnowledgeGraph>>>,
    default_tenant: Option<String>,
}

impl TenantKnowledgeGraphRegistry {
    pub fn new() -> Self {
        Self {
            tenant_graphs: Arc::new(dashmap::DashMap::new()),
            default_tenant: None,
        }
    }
    
    /// Register a tenant knowledge graph
    pub fn register_tenant(&self, tenant_id: String, graph: Arc<TenantKnowledgeGraph>) {
        self.tenant_graphs.insert(tenant_id, graph);
    }
    
    /// Get tenant knowledge graph
    pub fn get_tenant_graph(&self, tenant_id: &str) -> Option<Arc<TenantKnowledgeGraph>> {
        self.tenant_graphs.get(tenant_id).map(|entry| entry.clone())
    }
    
    /// Set default tenant for backward compatibility
    pub fn set_default_tenant(&mut self, tenant_id: String) {
        self.default_tenant = Some(tenant_id);
    }
    
    /// Get default tenant graph for legacy code compatibility
    pub fn get_default_tenant_graph(&self) -> Option<Arc<TenantKnowledgeGraph>> {
        if let Some(ref default_tenant) = self.default_tenant {
            self.get_tenant_graph(default_tenant)
        } else {
            None
        }
    }
}

impl Default for TenantKnowledgeGraphRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Global registry instance
static TENANT_REGISTRY: std::sync::OnceLock<TenantKnowledgeGraphRegistry> = std::sync::OnceLock::new();

/// Initialize tenant registry
pub fn initialize_tenant_registry() -> &'static TenantKnowledgeGraphRegistry {
    TENANT_REGISTRY.get_or_init(|| TenantKnowledgeGraphRegistry::new())
}

/// Get tenant registry
pub fn get_tenant_registry() -> Option<&'static TenantKnowledgeGraphRegistry> {
    TENANT_REGISTRY.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_knowledge_graph_factory() {
        let tenant_config = TenantConfig::default();
        let result = KnowledgeGraphFactory::create_tenant_graph("test_tenant", tenant_config).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_tenant_registry() {
        let registry = TenantKnowledgeGraphRegistry::new();
        
        // Should start empty
        assert!(registry.get_tenant_graph("nonexistent").is_none());
    }
}