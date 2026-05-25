//! Type definitions for multi-tenant knowledge graph architecture

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use chrono::{DateTime, Utc, Duration};

/// Tenant configuration for knowledge graph setup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantConfig {
    /// Compliance frameworks required for this tenant
    pub compliance_requirements: Vec<ComplianceFramework>,
    
    /// Default performance tier for tenant data
    pub default_performance_tier: PerformanceTier,
    
    /// Audit log retention period (days)
    pub audit_retention_days: u32,
    
    /// Data residency requirements
    pub data_residency: DataResidency,
    
    /// Encryption requirements
    pub encryption_requirements: EncryptionRequirements,
    
    /// Default domains to create for tenant
    pub default_domains: Vec<DomainConfig>,
    
    /// Tenant-specific performance settings
    pub performance_config: TenantPerformanceConfig,
}

/// Domain configuration within a tenant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainConfig {
    /// Domain name (unique within tenant)
    pub name: String,
    
    /// Business context and purpose
    pub business_context: BusinessContext,
    
    /// Domain-specific RBAC policies
    pub rbac_policies: Vec<DomainRBACPolicy>,
    
    /// Domain performance tier
    pub performance_tier: PerformanceTier,
    
    /// Domain-specific compliance requirements
    pub compliance_requirements: Vec<ComplianceFramework>,
    
    /// Default entity types for this domain
    pub default_entity_types: Vec<EntityTypeDefinition>,
    
    /// Default relationship types
    pub default_relationship_types: Vec<RelationshipTypeDefinition>,
}

/// Business context for domain understanding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessContext {
    /// Primary business function
    pub primary_function: String,
    
    /// Business domain category
    pub category: BusinessCategory,
    
    /// Regulatory environment
    pub regulatory_environment: Vec<RegulatoryFramework>,
    
    /// Data sensitivity level
    pub data_sensitivity: DataSensitivityLevel,
    
    /// Business criticality
    pub criticality: BusinessCriticality,
}

/// Performance tier for data and operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PerformanceTier {
    /// Hot data - memory/NVMe, <1ms latency
    Hot,
    /// Warm data - SSD, <10ms latency  
    Warm,
    /// Cold data - HDD/Cloud, <100ms latency
    Cold,
    /// Archive data - Glacier, <1s latency
    Archive,
}

/// Data residency requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DataResidency {
    /// No restrictions
    Global,
    /// Prefer specific region
    RegionPreferred(String),
    /// Lock to specific region
    RegionLocked(String),
    /// Strict local-only (HIPAA, financial)
    StrictLocal,
}

/// Encryption requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EncryptionRequirements {
    /// Standard TLS + AES-256
    Standard,
    /// FIPS 140-2 Level 2
    FIPS_140_2_Level_2,
    /// FIPS 140-2 Level 3 (HSM required)
    FIPS_140_2_Level_3,
    /// Custom encryption (defense, finance)
    Custom(CustomEncryptionConfig),
}

/// Compliance frameworks
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ComplianceFramework {
    SOC2,
    HIPAA,
    GDPR,
    BaselIII,
    SOX,
    FDA_CFR_Part11,
    ISO_27001,
    FedRAMP,
    Custom(String),
}

/// Business category classification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BusinessCategory {
    Financial,
    Healthcare,
    Government,
    Technology,
    Manufacturing,
    Retail,
    Education,
    Custom(String),
}

/// Data sensitivity levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub enum DataSensitivityLevel {
    Public,
    Internal,
    Confidential,
    Restricted,
    TopSecret,
}

/// Business criticality levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub enum BusinessCriticality {
    Low,
    Medium,
    High,
    Critical,
    MissionCritical,
}

/// Backwards-compat alias for [`KnowledgeGraphUserContext`].
pub type UserContext = KnowledgeGraphUserContext;

/// User context for RBAC validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeGraphUserContext {
    /// Unique user identifier
    pub user_id: String,
    
    /// Tenant the user belongs to
    pub tenant_id: String,
    
    /// User roles within tenant
    pub roles: Vec<String>,
    
    /// User permissions (computed from roles)
    pub permissions: HashSet<Permission>,
    
    /// Department/organization unit
    pub department: Option<String>,
    
    /// Security clearance level
    pub clearance_level: ClearanceLevel,
    
    /// Session context
    pub session_context: SessionContext,
    
    /// Industry-specific context (medical license, etc.)
    pub professional_context: Option<ProfessionalContext>,
}

/// Security clearance levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub enum ClearanceLevel {
    Public,
    Internal,
    Confidential,
    Secret,
    TopSecret,
    /// HIPAA-specific clearance for PHI access
    PHI_Authorized,
    /// Financial regulatory clearance
    FinancialRegulatory,
}

/// Session context for audit and security
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    pub session_id: String,
    pub login_timestamp: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub source_ip: String,
    pub user_agent: Option<String>,
    pub mfa_validated: bool,
}

/// Professional context for industry-specific validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProfessionalContext {
    Medical {
        license_number: String,
        license_state: String,
        specialization: Vec<String>,
        hospital_affiliation: Option<String>,
    },
    Financial {
        series_licenses: Vec<String>,
        institution: String,
        regulatory_oversight: Vec<String>,
    },
    Legal {
        bar_number: String,
        jurisdiction: String,
        practice_areas: Vec<String>,
    },
}

/// Permission enumeration for RBAC
#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub enum Permission {
    // Tenant-level permissions
    TenantAdmin,
    TenantRead,
    TenantWrite,
    
    // Domain-level permissions
    DomainAdmin(String),
    DomainRead(String),
    DomainWrite(String),
    DomainCreate,
    
    // Entity-level permissions
    EntityRead(String),
    EntityWrite(String),
    EntityDelete(String),
    EntityPII,
    
    // Relationship-level permissions
    RelationshipRead(String),
    RelationshipWrite(String),
    RelationshipTraverse(String),
    
    // Collection-level permissions
    CollectionRead(String),
    CollectionWrite(String),
    CollectionAdmin(String),
    
    // Special permissions
    CrossDomainQuery,
    AuditAccess,
    ComplianceReporting,
    SystemAdmin,
}

/// Cross-domain composition query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossDomainCompositionQuery {
    /// Primary domain for query
    pub primary_domain: String,
    
    /// Related domains to include
    pub related_domains: Vec<String>,
    
    /// Composition rules for knowledge fusion
    pub composition_rules: Vec<CompositionRule>,
    
    /// Query filters
    pub filters: CompositionFilters,
    
    /// Expected result format
    pub result_format: CompositionResultFormat,
    
    /// Privacy and security controls
    pub privacy_controls: PrivacyControls,
}

/// Rules for composing knowledge across domains
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompositionRule {
    /// Correlate entities across domains
    CorrelateEntities {
        correlation_field: String,
        correlation_strength: f32,
        max_hops: u32,
    },
    
    /// Enrich with relationships
    EnrichWithRelationships {
        relationship_types: Vec<String>,
        include_indirect: bool,
        max_depth: u32,
    },
    
    /// Aggregate metrics across domains
    AggregateMetrics {
        metric_fields: Vec<String>,
        aggregation_function: AggregationFunction,
        time_window: Option<Duration>,
    },
    
    /// Infer new relationships
    InferRelationships {
        inference_rules: Vec<InferenceRule>,
        confidence_threshold: f32,
    },
    
    /// Custom business rule
    CustomRule {
        rule_name: String,
        rule_logic: String, // Could be code or configuration
        parameters: HashMap<String, serde_json::Value>,
    },
}

/// Privacy controls for knowledge composition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyControls {
    /// Require anonymization of PII
    pub anonymization_required: bool,
    
    /// Log all access for audit
    pub audit_trail_required: bool,
    
    /// Validate user consent for data use
    pub consent_validation_required: bool,
    
    /// Purpose for accessing the data
    pub access_purpose: AccessPurpose,
    
    /// Data minimization - only return necessary fields
    pub apply_data_minimization: bool,
    
    /// Retention period for composed knowledge
    pub retention_period: Option<Duration>,
}

/// Purpose for data access (regulatory requirement)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccessPurpose {
    DirectCustomerService,
    BusinessAnalytics,
    RegulatoryCompliance,
    SecurityInvestigation,
    SystemMaintenance,
    ResearchAndDevelopment,
    Custom(String),
}

/// Collection link configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionLinkConfiguration {
    /// Automatically create entities for new vectors
    pub auto_entity_creation: bool,
    
    /// Default entity type for auto-created entities
    pub default_entity_type: String,
    
    /// Relationship inference policy
    pub relationship_inference: RelationshipInferencePolicy,
    
    /// Provenance tracking level
    pub provenance_tracking: ProvenanceTrackingPolicy,
    
    /// Field mapping from vector metadata to entity fields
    pub field_mapping: FieldMappingConfig,
    
    /// Synchronization mode
    pub sync_mode: SyncMode,
    
    /// Privacy controls for this collection
    pub privacy_controls: PrivacyControls,
}

/// Relationship inference policies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RelationshipInferencePolicy {
    /// No automatic inference
    None,
    /// Conservative inference with high confidence
    Conservative,
    /// Aggressive inference with medium confidence
    Aggressive,
    /// Custom inference rules
    Custom(Vec<InferenceRule>),
}

/// Provenance tracking policies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProvenanceTrackingPolicy {
    /// No provenance tracking
    None,
    /// Basic metadata tracking
    Metadata,
    /// Full provenance chain tracking
    Full,
    /// Custom provenance rules
    Custom(ProvenanceConfig),
}

/// Synchronization modes between collections and entities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMode {
    /// Real-time synchronization
    Realtime,
    /// Batch synchronization
    Batch { interval_seconds: u32 },
    /// Manual synchronization
    Manual,
    /// Event-driven synchronization
    EventDriven,
}

/// Field mapping configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldMappingConfig {
    /// Vector metadata field → Entity field mappings
    pub field_mappings: HashMap<String, String>,
    
    /// Default values for missing fields
    pub default_values: HashMap<String, serde_json::Value>,
    
    /// Field transformation rules
    pub transformations: Vec<FieldTransformation>,
    
    /// Validation rules for mapped fields
    pub validation_rules: Vec<FieldValidationRule>,
}

/// Field transformation rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldTransformation {
    pub source_field: String,
    pub target_field: String,
    pub transformation_type: TransformationType,
    pub parameters: HashMap<String, serde_json::Value>,
}

/// Types of field transformations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransformationType {
    DirectMapping,
    TypeConversion,
    ValueNormalization,
    Anonymization,
    Encryption,
    Custom(String),
}

/// Composed knowledge result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposedKnowledge {
    /// Primary domain results
    pub primary_results: DomainKnowledgeResult,
    
    /// Related domain results
    pub related_results: HashMap<String, DomainKnowledgeResult>,
    
    /// Cross-domain correlations discovered
    pub correlations: Vec<CrossDomainCorrelation>,
    
    /// Composition metadata
    pub composition_metadata: CompositionMetadata,
    
    /// Full provenance chain
    pub provenance: CompositionProvenance,
    
    /// RBAC enforcement summary
    pub rbac_summary: RBACEnforcementSummary,
}

/// Domain-specific knowledge result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainKnowledgeResult {
    pub domain_id: String,
    pub entities: Vec<SecureEntity>,
    pub relationships: Vec<SecureRelationship>,
    pub domain_context: DomainContext,
    pub query_metadata: QueryMetadata,
}

/// Secure entity with RBAC filtering applied
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecureEntity {
    /// Original entity with filtered fields
    pub entity: crate::proto::proximadb_v1::Entity,
    
    /// Access metadata
    pub access_metadata: EntityAccessMetadata,
    
    /// Filtered fields (for audit)
    pub filtered_fields: Vec<String>,
    
    /// Security classification
    pub security_classification: SecurityClassification,
}

/// Cross-domain correlation discovered during composition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossDomainCorrelation {
    pub source_domain: String,
    pub target_domain: String,
    pub correlation_type: CorrelationType,
    pub confidence_score: f32,
    pub entities_involved: Vec<String>,
    pub discovered_at: DateTime<Utc>,
}

/// Migration plan from global to tenant-aware architecture
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalToTenantMigrationPlan {
    /// Tenant migration configurations
    pub tenant_migrations: Vec<TenantMigration>,
    
    /// Global entity assignments to tenants/domains
    pub entity_assignments: HashMap<String, EntityAssignment>,
    
    /// Relationship migration strategy
    pub relationship_migration_strategy: RelationshipMigrationStrategy,
    
    /// Backward compatibility settings
    pub backward_compatibility: BackwardCompatibilityConfig,
    
    /// Migration validation criteria
    pub validation_criteria: MigrationValidationCriteria,
}

/// Individual tenant migration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantMigration {
    pub tenant_id: String,
    pub tenant_config: TenantConfig,
    pub domain_migrations: Vec<DomainMigration>,
    pub collection_assignments: HashMap<String, String>, // collection_id -> domain_name
}

/// Domain migration within tenant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainMigration {
    pub domain_name: String,
    pub domain_config: DomainConfig,
    pub entity_migrations: Vec<EntityMigration>,
    pub relationship_migrations: Vec<RelationshipMigration>,
}

/// Entity assignment to tenant/domain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityAssignment {
    pub tenant_id: String,
    pub domain_name: String,
    pub entity_permissions: EntityPermissions,
    pub migration_priority: MigrationPriority,
}

impl Default for TenantConfig {
    fn default() -> Self {
        Self {
            compliance_requirements: vec![ComplianceFramework::SOC2],
            default_performance_tier: PerformanceTier::Warm,
            audit_retention_days: 2555, // 7 years
            data_residency: DataResidency::Global,
            encryption_requirements: EncryptionRequirements::Standard,
            default_domains: vec![
                DomainConfig {
                    name: "default".to_string(),
                    business_context: BusinessContext {
                        primary_function: "general".to_string(),
                        category: BusinessCategory::Technology,
                        regulatory_environment: vec![],
                        data_sensitivity: DataSensitivityLevel::Internal,
                        criticality: BusinessCriticality::Medium,
                    },
                    rbac_policies: vec![],
                    performance_tier: PerformanceTier::Warm,
                    compliance_requirements: vec![ComplianceFramework::SOC2],
                    default_entity_types: vec![],
                    default_relationship_types: vec![],
                },
            ],
            performance_config: TenantPerformanceConfig::default(),
        }
    }
}

impl Default for DomainConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            business_context: BusinessContext {
                primary_function: "general".to_string(),
                category: BusinessCategory::Technology,
                regulatory_environment: vec![],
                data_sensitivity: DataSensitivityLevel::Internal,
                criticality: BusinessCriticality::Medium,
            },
            rbac_policies: vec![],
            performance_tier: PerformanceTier::Warm,
            compliance_requirements: vec![],
            default_entity_types: vec![],
            default_relationship_types: vec![],
        }
    }
}

impl KnowledgeGraphUserContext {
    /// Create system admin context for internal operations
    pub fn system_admin() -> Self {
        Self {
            user_id: "system".to_string(),
            tenant_id: "system".to_string(),
            roles: vec!["system_admin".to_string()],
            permissions: [Permission::SystemAdmin].into_iter().collect(),
            department: None,
            clearance_level: ClearanceLevel::TopSecret,
            session_context: SessionContext {
                session_id: "system_session".to_string(),
                login_timestamp: Utc::now(),
                last_activity: Utc::now(),
                source_ip: "localhost".to_string(),
                user_agent: Some("ProximaDB System".to_string()),
                mfa_validated: true,
            },
            professional_context: None,
        }
    }
    
    /// Check if user has specific permission
    pub fn has_permission(&self, permission: &Permission) -> bool {
        self.permissions.contains(permission) || 
        self.permissions.contains(&Permission::SystemAdmin)
    }
    
    /// Check if user can access specific domain
    pub fn can_access_domain(&self, domain_name: &str) -> bool {
        self.has_permission(&Permission::DomainRead(domain_name.to_string())) ||
        self.has_permission(&Permission::DomainAdmin(domain_name.to_string())) ||
        self.has_permission(&Permission::TenantAdmin)
    }
}

// Additional type definitions for completeness...

/// Tenant performance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantPerformanceConfig {
    pub max_concurrent_queries: u32,
    pub memory_limit_mb: u32,
    pub query_timeout_seconds: u32,
    pub cache_size_mb: u32,
}

impl Default for TenantPerformanceConfig {
    fn default() -> Self {
        Self {
            max_concurrent_queries: 1000,
            memory_limit_mb: 4096,
            query_timeout_seconds: 30,
            cache_size_mb: 1024,
        }
    }
}

/// Additional placeholder types for comprehensive specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityTypeDefinition {
    pub name: String,
    pub schema: HashMap<String, FieldDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipTypeDefinition {
    pub name: String,
    pub source_entity_types: Vec<String>,
    pub target_entity_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDefinition {
    pub field_type: String,
    pub required: bool,
    pub privacy_level: DataSensitivityLevel,
}

// Placeholder for additional complex types
pub type DomainRBACPolicy = String; // Will be fully implemented
pub type RegulatoryFramework = String;
pub type CustomEncryptionConfig = String;
pub type EntityPermissions = String;
pub type MigrationPriority = String;
pub type RelationshipMigration = String;
pub type EntityMigration = String;
pub type RelationshipMigrationStrategy = String;
pub type BackwardCompatibilityConfig = String;
pub type MigrationValidationCriteria = String;
pub type CompositionFilters = String;
pub type CompositionResultFormat = String;
pub type CorrelationType = String;
pub type AggregationFunction = String;
pub type InferenceRule = String;
pub type EntityAccessMetadata = String;
pub type SecurityClassification = String;
pub type DomainContext = String;
pub type QueryMetadata = String;
pub type CompositionMetadata = String;
pub type CompositionProvenance = String;
pub type RBACEnforcementSummary = String;
pub type FieldValidationRule = String;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_config_default() {
        let config = TenantConfig::default();
        assert_eq!(config.compliance_requirements, vec![ComplianceFramework::SOC2]);
        assert_eq!(config.default_performance_tier, PerformanceTier::Warm);
        assert!(!config.default_domains.is_empty());
    }

    #[test]
    fn test_user_context_permissions() {
        let user = KnowledgeGraphUserContext::system_admin();
        assert!(user.has_permission(&Permission::SystemAdmin));
        assert!(user.can_access_domain("any_domain"));
    }

    #[test]
    fn test_clearance_level_ordering() {
        assert!(ClearanceLevel::TopSecret > ClearanceLevel::Confidential);
        assert!(ClearanceLevel::Confidential > ClearanceLevel::Internal);
        assert!(ClearanceLevel::Internal > ClearanceLevel::Public);
    }

    #[test]
    fn test_performance_tier_classification() {
        let hot = PerformanceTier::Hot;
        let warm = PerformanceTier::Warm;
        let cold = PerformanceTier::Cold;
        
        assert_ne!(hot, warm);
        assert_ne!(warm, cold);
        assert_ne!(hot, cold);
    }
}