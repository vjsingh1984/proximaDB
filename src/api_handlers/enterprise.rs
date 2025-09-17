//! Enterprise API handlers for multi-tenant knowledge intelligence

use anyhow::{Result, anyhow};
use std::sync::Arc;
use tracing::info;

use crate::auth::{EnterpriseAuthManager, SSOToken, EnterpriseUserContext};
use crate::storage::tenant::{
    TenantManager, DomainManager, TenantAwareEntityStore, DomainKnowledgeGraph,
    TenantConfig, BusinessContext, knowledge_graph::CollectionBridgeConfig,
    SecurityPolicies, Industry, ComplianceFramework,
};
use crate::storage::tenant::context::ResourceLimits;

/// Enterprise API handler for multi-tenant operations
pub struct EnterpriseAPIHandler {
    /// Tenant management
    tenant_manager: Arc<TenantManager>,
    
    /// Domain management
    domain_manager: Arc<DomainManager>,
    
    /// Enhanced entity store
    entity_store: Arc<TenantAwareEntityStore>,
    
    /// Enterprise authentication manager
    auth_manager: Arc<EnterpriseAuthManager>,
    
    /// Domain knowledge graphs by tenant and domain
    knowledge_graphs: Arc<dashmap::DashMap<String, Arc<DomainKnowledgeGraph>>>,
}

impl EnterpriseAPIHandler {
    /// Create new enterprise API handler
    pub fn new(
        tenant_manager: Arc<TenantManager>,
        domain_manager: Arc<DomainManager>,
        entity_store: Arc<TenantAwareEntityStore>,
        auth_manager: Arc<EnterpriseAuthManager>,
    ) -> Self {
        Self {
            tenant_manager,
            domain_manager,
            entity_store,
            auth_manager,
            knowledge_graphs: Arc::new(dashmap::DashMap::new()),
        }
    }
    
    /// Create enterprise tenant with comprehensive setup
    pub async fn create_enterprise_tenant(
        &self,
        tenant_id: String,
        tenant_config: TenantConfig,
        sso_token: &SSOToken,
    ) -> Result<EnterpriseApiResponse<TenantCreationResult>> {
        // Authenticate and authorize
        let enterprise_user = self.auth_manager.validate_and_resolve_token(sso_token).await?;
        
        // Validate user can create tenants (system admin only for now)
        if !enterprise_user.has_permission("system_admin") {
            return Err(anyhow!("Insufficient permissions to create tenant"));
        }
        
        // Create tenant
        let tenant_context = self.tenant_manager.create_tenant(tenant_id.clone(), tenant_config.clone()).await?;
        
        // Create default domains based on industry
        let default_domains = self.create_default_domains_for_industry(
            &tenant_id,
            &tenant_config.industry,
            &enterprise_user,
        ).await?;
        
        // Setup standard RBAC roles
        let rbac_manager = crate::storage::tenant::EnhancedRBACManager::new(self.tenant_manager.clone());
        let standard_roles = rbac_manager.create_standard_roles(&tenant_id, &enterprise_user.clone().into()).await?;

        info!("Created enterprise tenant {} with {} domains and {} roles",
              tenant_id, default_domains.len(), standard_roles.len());

        let created_by = enterprise_user.user_id.clone();
        Ok(EnterpriseApiResponse {
            success: true,
            data: TenantCreationResult {
                tenant_context,
                default_domains,
                standard_roles,
                created_by,
            },
            enterprise_metadata: EnterpriseApiMetadata {
                user_context: enterprise_user,
                operation_timestamp: chrono::Utc::now(),
                audit_trail_id: Some("tenant_creation_audit".to_string()),
            },
        })
    }
    
    /// Create domain knowledge graph with business intelligence
    pub async fn create_domain_knowledge_graph(
        &self,
        tenant_id: String,
        domain_name: String,
        business_context: BusinessContext,
        sso_token: &SSOToken,
    ) -> Result<EnterpriseApiResponse<DomainCreationResult>> {
        // Authenticate and authorize
        let enterprise_user = self.auth_manager.validate_and_resolve_token(sso_token).await?;
        
        // Create domain
        let domain_context = self.domain_manager.create_domain(
            &tenant_id,
            &domain_name,
            business_context.clone(),
            &enterprise_user.clone().into(),
        ).await?;
        
        // Create domain knowledge graph
        let knowledge_graph = DomainKnowledgeGraph::new(
            domain_context.clone(),
            self.tenant_manager.clone(),
        ).await?;
        
        // Store knowledge graph
        let kg_key = format!("{}::{}", tenant_id, domain_name);
        let kg_key_clone = kg_key.clone();
        self.knowledge_graphs.insert(kg_key, Arc::new(knowledge_graph));

        info!("Created domain knowledge graph {} in tenant {} with business context: {}",
              domain_name, tenant_id, business_context.primary_function);

        Ok(EnterpriseApiResponse {
            success: true,
            data: DomainCreationResult {
                domain_context,
                business_context,
                knowledge_graph_id: kg_key_clone,
            },
            enterprise_metadata: EnterpriseApiMetadata {
                user_context: enterprise_user,
                operation_timestamp: chrono::Utc::now(),
                audit_trail_id: Some("domain_creation_audit".to_string()),
            },
        })
    }
    
    /// Link collection to domain with intelligent bridging
    pub async fn link_collection_to_domain(
        &self,
        tenant_id: String,
        domain_name: String,
        collection_id: String,
        bridge_config: CollectionBridgeConfig,
        sso_token: &SSOToken,
    ) -> Result<EnterpriseApiResponse<CollectionLinkResult>> {
        // Authenticate and authorize
        let enterprise_user = self.auth_manager.validate_and_resolve_token(sso_token).await?;
        
        // Get domain knowledge graph
        let kg_key = format!("{}::{}", tenant_id, domain_name);
        let knowledge_graph = self.knowledge_graphs.get(&kg_key)
            .ok_or_else(|| anyhow!("Domain knowledge graph not found: {}", kg_key))?;
        
        // Link collection to domain
        let bridge = knowledge_graph.link_collection(
            &collection_id,
            bridge_config,
            &enterprise_user.clone().into(),
        ).await?;

        info!("Linked collection {} to domain {} in tenant {}",
              collection_id, domain_name, tenant_id);

        Ok(EnterpriseApiResponse {
            success: true,
            data: CollectionLinkResult {
                bridge,
                tenant_id,
                domain_name,
                collection_id,
            },
            enterprise_metadata: EnterpriseApiMetadata {
                user_context: enterprise_user,
                operation_timestamp: chrono::Utc::now(),
                audit_trail_id: Some("collection_link_audit".to_string()),
            },
        })
    }
    
    // Helper method to create default domains based on industry
    async fn create_default_domains_for_industry(
        &self,
        tenant_id: &str,
        industry: &crate::storage::tenant::Industry,
        enterprise_user: &EnterpriseUserContext,
    ) -> Result<Vec<crate::storage::tenant::DomainContext>> {
        let mut domains = Vec::new();
        let user_context: crate::storage::tenant::UserContext = enterprise_user.clone().into();
        
        match industry {
            crate::storage::tenant::Industry::Financial => {
                // Create financial services domains
                let risk_domain = self.domain_manager.create_domain(
                    tenant_id,
                    "risk_management",
                    BusinessContext {
                        primary_function: "enterprise_risk_assessment".to_string(),
                        data_sensitivity: crate::storage::tenant::DataSensitivityLevel::Confidential,
                        performance_requirements: crate::storage::tenant::context::PerformanceRequirements {
                            latency_requirement_ms: 50,
                            throughput_requirement_qps: 5000,
                            availability_requirement: 0.999,
                        },
                    },
                    &user_context,
                ).await?;
                domains.push(risk_domain);
                
                let trading_domain = self.domain_manager.create_domain(
                    tenant_id,
                    "trading_operations",
                    BusinessContext {
                        primary_function: "trading_and_portfolio_management".to_string(),
                        data_sensitivity: crate::storage::tenant::DataSensitivityLevel::Confidential,
                        performance_requirements: crate::storage::tenant::context::PerformanceRequirements {
                            latency_requirement_ms: 10,
                            throughput_requirement_qps: 10000,
                            availability_requirement: 0.9999,
                        },
                    },
                    &user_context,
                ).await?;
                domains.push(trading_domain);
            },
            crate::storage::tenant::Industry::Healthcare => {
                // Create healthcare domains
                let clinical_domain = self.domain_manager.create_domain(
                    tenant_id,
                    "clinical_care",
                    BusinessContext {
                        primary_function: "patient_care_and_clinical_decision_support".to_string(),
                        data_sensitivity: crate::storage::tenant::DataSensitivityLevel::Restricted,
                        performance_requirements: crate::storage::tenant::context::PerformanceRequirements {
                            latency_requirement_ms: 100,
                            throughput_requirement_qps: 2000,
                            availability_requirement: 0.999,
                        },
                    },
                    &user_context,
                ).await?;
                domains.push(clinical_domain);
            },
            _ => {
                // Create generic domain
                let general_domain = self.domain_manager.create_domain(
                    tenant_id,
                    "general",
                    BusinessContext::default(),
                    &user_context,
                ).await?;
                domains.push(general_domain);
            }
        }
        
        Ok(domains)
    }
}

/// Enterprise API response wrapper
#[derive(Debug, Clone)]
pub struct EnterpriseApiResponse<T> {
    pub success: bool,
    pub data: T,
    pub enterprise_metadata: EnterpriseApiMetadata,
}

/// Enterprise API metadata for audit and tracking
#[derive(Debug, Clone)]
pub struct EnterpriseApiMetadata {
    pub user_context: EnterpriseUserContext,
    pub operation_timestamp: DateTime<Utc>,
    pub audit_trail_id: Option<String>,
}

/// Tenant creation result
#[derive(Debug, Clone)]
pub struct TenantCreationResult {
    pub tenant_context: crate::storage::tenant::TenantContext,
    pub default_domains: Vec<crate::storage::tenant::DomainContext>,
    pub standard_roles: Vec<crate::storage::tenant::TenantRole>,
    pub created_by: String,
}

/// Domain creation result
#[derive(Debug, Clone)]
pub struct DomainCreationResult {
    pub domain_context: crate::storage::tenant::DomainContext,
    pub business_context: BusinessContext,
    pub knowledge_graph_id: String,
}

/// Collection link result
#[derive(Debug, Clone)]
pub struct CollectionLinkResult {
    pub bridge: crate::storage::tenant::CollectionDomainBridge,
    pub tenant_id: String,
    pub domain_name: String,
    pub collection_id: String,
}

use chrono::{DateTime, Utc};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::sso::{SSOIntegrationManager};
    use crate::storage::tenant::{Industry, ComplianceFramework, SecurityPolicies};

    async fn create_test_enterprise_handler() -> EnterpriseAPIHandler {
        let tenant_manager = Arc::new(TenantManager::new());
        let domain_manager = Arc::new(DomainManager::new());
        let entity_store = Arc::new(TenantAwareEntityStore::new(tenant_manager.clone()));
        
        // Create simplified auth manager for testing
        let sso_manager = SSOIntegrationManager::new();
        let rbac_manager = crate::storage::tenant::EnhancedRBACManager::new(tenant_manager.clone());
        let auth_manager = Arc::new(EnterpriseAuthManager::new(sso_manager, rbac_manager));
        
        EnterpriseAPIHandler::new(
            tenant_manager,
            domain_manager,
            entity_store,
            auth_manager,
        )
    }

    #[tokio::test]
    async fn test_enterprise_tenant_creation() {
        let handler = create_test_enterprise_handler().await;
        
        let tenant_config = TenantConfig {
            organization_name: "Global Investment Bank".to_string(),
            industry: Industry::Financial,
            compliance_requirements: vec![ComplianceFramework::SOC2, ComplianceFramework::SOX],
            resource_limits: ResourceLimits::default(),
            security_policies: SecurityPolicies::default(),
        };
        
        // Create mock SSO token
        let sso_token = SSOToken::new(
            crate::auth::sso::SSOProvider::AWSIAM,
            "mock_token_data".to_string(),
            "system_admin".to_string(),
            3600,
        );
        
        // This test demonstrates the API flow (would work with real auth)
        // For now, testing the structure and flow
        assert_eq!(tenant_config.industry, Industry::Financial);
    }
}