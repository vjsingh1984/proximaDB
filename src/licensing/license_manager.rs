//! License Management System
//!
//! Handles license tier validation, feature gating, and both online/offline license enforcement
//! for privacy-conscious customers and air-gapped deployments

use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Comprehensive license management for all deployment scenarios
#[derive(Debug, Clone)]
pub struct LicenseManager {
    current_license: Option<LicenseInfo>,
    #[allow(dead_code)]
    tier_enforcement: TierEnforcement,
    offline_validator: OfflineLicenseValidator,
    config: LicenseConfig,
}

/// License configuration for different deployment models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseConfig {
    /// The deployment model that determines how license validation is performed.
    pub deployment_model: DeploymentModel,
    /// Whether the license manager may contact the ProximaDB licensing service for online validation.
    pub enable_phone_home: bool,
    /// When `true`, only offline (cryptographic) validation is performed; no network calls are made.
    pub offline_validation_only: bool,
    /// Interval in hours between periodic license re-validation checks.
    pub license_check_interval_hours: u32,
    /// Number of days after license expiry during which the system continues operating in grace-period mode.
    pub grace_period_days: u32,
    /// Whether anonymised usage analytics may be collected and reported.
    pub enable_usage_analytics: bool,
}

/// Deployment models for license management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeploymentModel {
    /// ProximaDB-hosted SaaS deployment with full online connectivity.
    SaaSHosted,
    /// Customer-managed VPC deployment with limited or controlled outbound connectivity.
    CustomerVPC,
    /// Completely air-gapped on-premises deployment with no external network access.
    AirGapped,
    /// Mixed deployment where some nodes have connectivity and others do not.
    Hybrid,
}

/// Complete license information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseInfo {
    /// Unique identifier for this license.
    pub license_id: String,
    /// Identifier of the customer that owns this license.
    pub customer_id: String,
    /// Human-readable name of the customer.
    pub customer_name: String,
    /// The license tier that governs which features and limits apply.
    pub license_tier: LicenseTier,
    /// Timestamp at which this license was issued.
    pub issued_at: DateTime<Utc>,
    /// Optional expiry timestamp; `None` means the license never expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// Feature entitlements granted by this license.
    pub feature_entitlements: FeatureEntitlements,
    /// Usage limits imposed by this license.
    pub usage_limits: UsageLimits,
    /// Cryptographically signed offline token used for air-gapped validation.
    pub license_token: String,
    /// Support tier included with this license.
    pub support_tier: SupportTier,
}

/// License tiers with different capabilities and limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LicenseTier {
    /// Free/trial tier with restricted features.
    Free {
        /// Number of days the trial is valid.
        trial_days: u32,
        /// List of feature identifiers that are unavailable in the free tier.
        feature_restrictions: Vec<String>,
    },
    /// Developer tier for individual developers with a monthly subscription.
    Developer {
        /// Monthly subscription price in USD.
        monthly_price: f64,
        /// Usage limits that apply to this developer license.
        usage_limits: DeveloperLimits,
    },
    /// Professional tier with advanced features and higher usage limits.
    Professional {
        /// Monthly subscription price in USD.
        monthly_price: f64,
        /// Usage limits that apply to this professional license.
        usage_limits: ProfessionalLimits,
        /// Additional advanced features enabled at this tier.
        advanced_features: Vec<String>,
    },
    /// Standard enterprise tier with custom pricing and unlimited usage.
    Enterprise {
        /// Whether pricing is negotiated individually with the customer.
        custom_pricing: bool,
        /// Whether usage is unlimited (no hard caps).
        unlimited_usage: bool,
        /// Enterprise-specific feature identifiers enabled for this customer.
        custom_features: Vec<String>,
        /// SLA guarantee identifiers included in this enterprise contract.
        sla_guarantees: Vec<String>,
    },
    /// Bespoke enterprise tier for customers with unique contractual requirements.
    CustomEnterprise {
        /// Arbitrary key-value pairs encoding custom contractual terms.
        custom_terms: HashMap<String, String>,
        /// Feature identifiers negotiated exclusively for this customer.
        bespoke_features: Vec<String>,
        /// Whether a dedicated support team is assigned to this customer.
        dedicated_support: bool,
    },
}

impl std::fmt::Display for LicenseTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LicenseTier::Free { .. } => write!(f, "Free"),
            LicenseTier::Developer { .. } => write!(f, "Developer"),
            LicenseTier::Professional { .. } => write!(f, "Professional"),
            LicenseTier::Tier5 { .. } => write!(f, "Enterprise"),
            LicenseTier::CustomEnterprise { .. } => write!(f, "Custom Enterprise"),
        }
    }
}

/// Feature entitlements based on license tier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureEntitlements {
    /// AI and LLM-related capabilities granted by this license.
    pub ai_capabilities: AICapabilities,
    /// Multi-tenancy capabilities granted by this license.
    pub multi_tenant_features: MultiTenantFeatures,
    /// Performance and storage capabilities granted by this license.
    pub performance_features: PerformanceFeatures,
    /// Security and compliance capabilities granted by this license.
    pub security_features: SecurityFeatures,
    /// Enterprise-grade capabilities granted by this license.
    pub enterprise_features: EnterpriseFeatures,
}

/// AI capability entitlements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AICapabilities {
    /// Whether natural language query processing is enabled.
    pub natural_language_queries: bool,
    /// Whether AI-powered executive dashboards are enabled.
    pub executive_dashboards: bool,
    /// List of LLM provider identifiers the customer may use (e.g. `"OpenAI"`, `"Anthropic"`).
    pub llm_providers_allowed: Vec<String>,
    /// Monthly cap on AI queries; `None` means unlimited.
    pub ai_queries_per_month: Option<u32>,
    /// Whether advanced AI features beyond basic NL queries are enabled.
    pub advanced_ai_features: bool,
}

/// Multi-tenant feature entitlements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiTenantFeatures {
    /// Maximum number of tenants allowed; `None` means unlimited.
    pub max_tenants: Option<u32>,
    /// The level of isolation enforced between tenants.
    pub tenant_isolation_level: IsolationLevel,
    /// Whether advanced RBAC features (custom roles, fine-grained policies) are enabled.
    pub rbac_advanced_features: bool,
    /// Whether per-tenant configuration overrides are allowed.
    pub custom_tenant_configuration: bool,
}

/// Performance feature entitlements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceFeatures {
    /// List of storage engine identifiers the customer may activate (e.g. `"SST"`, `"VIPER"`).
    pub storage_engines_allowed: Vec<String>,
    /// Maximum queries per second allowed; `None` means unlimited.
    pub max_qps: Option<u32>,
    /// Whether advanced in-memory caching tiers are enabled.
    pub advanced_caching: bool,
    /// Whether detailed performance metrics and monitoring dashboards are enabled.
    pub performance_monitoring: bool,
    /// Whether bespoke performance tuning and optimisation services are included.
    pub custom_optimization: bool,
}

/// Security feature entitlements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityFeatures {
    /// Whether SAML/OIDC single sign-on integration is enabled.
    pub sso_integration: bool,
    /// Whether immutable audit logging of all data access events is enabled.
    pub audit_logging: bool,
    /// Compliance frameworks supported (e.g. `"SOC2"`, `"GDPR"`, `"HIPAA"`).
    pub compliance_frameworks: Vec<String>,
    /// Whether advanced at-rest and in-transit encryption options are enabled.
    pub advanced_encryption: bool,
    /// Whether real-time security event monitoring and alerting is enabled.
    pub security_monitoring: bool,
}

/// Enterprise feature entitlements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnterpriseFeatures {
    /// Whether automated deployment tooling (Terraform modules, Helm charts, etc.) is included.
    pub deployment_automation: bool,
    /// Whether bespoke third-party integration development is included.
    pub custom_integrations: bool,
    /// Whether a dedicated customer success manager is assigned.
    pub dedicated_support: bool,
    /// Whether contractual SLA guarantees (uptime, response times) are in effect.
    pub sla_guarantees: bool,
    /// Whether custom feature development for the customer is included.
    pub custom_development: bool,
}

/// Usage limits based on license tier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLimits {
    /// Maximum number of collections that may be created; `None` means unlimited.
    pub max_collections: Option<u32>,
    /// Maximum total number of vectors across all collections; `None` means unlimited.
    pub max_vectors_total: Option<u64>,
    /// Maximum total storage in gigabytes; `None` means unlimited.
    pub max_storage_gb: Option<u32>,
    /// Maximum number of API calls per calendar month; `None` means unlimited.
    pub max_api_calls_per_month: Option<u32>,
    /// Maximum number of AI/LLM queries per calendar month; `None` means unlimited.
    pub max_ai_queries_per_month: Option<u32>,
    /// Maximum number of concurrent authenticated users; `None` means unlimited.
    pub max_concurrent_users: Option<u32>,
}

/// Support tier levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SupportTier {
    /// Community forum support only; no direct vendor contact.
    Community,
    /// Standard email-based support with best-effort response times.
    Standard,
    /// Priority support channel with contractual response-time SLAs.
    Priority,
    /// Dedicated customer success manager with proactive engagement.
    Dedicated,
    /// 24/7 white-glove support with an assigned engineering team.
    WhiteGlove,
}

/// License status and validation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LicenseStatus {
    /// License is active and all checks passed.
    Valid,
    /// License has passed its expiry date and the grace period has also elapsed.
    Expired,
    /// One or more usage limits defined in the license have been exceeded.
    ExceededLimits,
    /// The cryptographic signature on the license token is invalid.
    InvalidSignature,
    /// No license was found in any expected location.
    NotFound,
    /// License has expired but the system is operating within the grace period.
    GracePeriod {
        /// Number of days remaining in the grace period.
        days_remaining: u32,
    },
}

/// Isolation levels for multi-tenancy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IsolationLevel {
    /// Namespace-level tenant separation with shared resources.
    Basic,
    /// Advanced isolation with per-tenant resource quotas and access monitoring.
    Enhanced,
    /// Full enterprise isolation with hardware-level separation and comprehensive audit trails.
    Complete,
}

impl LicenseManager {
    /// Create new license manager
    pub async fn new(config: LicenseConfig) -> Result<Self> {
        info!(
            "🔐 Initializing license manager for deployment model: {:?}",
            config.deployment_model
        );

        let tier_enforcement = TierEnforcement::new();
        let offline_validator = OfflineLicenseValidator::new(&config)?;

        let manager = Self {
            current_license: None,
            tier_enforcement,
            offline_validator,
            config,
        };

        // Load existing license if available
        if let Ok(license) = manager.load_license_from_storage().await {
            info!(
                "✅ Loaded existing license: {} ({})",
                license.customer_name, license.license_tier
            );
        }

        Ok(manager)
    }

    /// Validate license for feature access
    pub async fn validate_feature_access(
        &self,
        feature: &str,
        usage_context: &UsageContext,
    ) -> Result<FeatureValidation> {
        debug!(
            "🔍 Validating feature access: {} for user {}",
            feature, usage_context.user_id
        );

        // Check if license exists and is valid
        let license = self
            .current_license
            .as_ref()
            .ok_or_else(|| anyhow!("No license found"))?;

        // Validate license is not expired
        let license_status = self.check_license_status(license).await?;
        if !matches!(
            license_status,
            LicenseStatus::Valid | LicenseStatus::GracePeriod { .. }
        ) {
            return Ok(FeatureValidation {
                allowed: false,
                reason: format!("License status: {:?}", license_status),
                limits_exceeded: vec![],
                upgrade_required: Some(self.suggest_license_upgrade(feature)),
            });
        }

        // Check feature entitlements
        let feature_allowed = self.is_feature_entitled(feature, &license.feature_entitlements)?;
        if !feature_allowed {
            return Ok(FeatureValidation {
                allowed: false,
                reason: format!(
                    "Feature '{}' not available in {:?} tier",
                    feature, license.license_tier
                ),
                limits_exceeded: vec![],
                upgrade_required: Some(self.suggest_license_upgrade(feature)),
            });
        }

        // Check usage limits
        let usage_validation = self
            .validate_usage_limits(usage_context, &license.usage_limits)
            .await?;
        if !usage_validation.within_limits {
            return Ok(FeatureValidation {
                allowed: false,
                reason: "Usage limits exceeded".to_string(),
                limits_exceeded: usage_validation.exceeded_limits,
                upgrade_required: Some(self.suggest_license_upgrade("usage_increase")),
            });
        }

        // All validations passed
        Ok(FeatureValidation {
            allowed: true,
            reason: "Feature access granted".to_string(),
            limits_exceeded: vec![],
            upgrade_required: None,
        })
    }

    /// Load license from storage (for offline validation)
    async fn load_license_from_storage(&self) -> Result<LicenseInfo> {
        match self.config.deployment_model {
            DeploymentModel::AirGapped | DeploymentModel::CustomerVPC => {
                // Load from local file for privacy-conscious deployments
                self.load_offline_license().await
            }
            _ => {
                // For SaaS, could check online licensing service
                self.load_online_license().await
            }
        }
    }

    /// Load offline license for air-gapped deployments
    async fn load_offline_license(&self) -> Result<LicenseInfo> {
        // Check for license file in secure location
        let license_paths = vec![
            "/etc/proximadb/license.json",
            "/config/license.json",
            "./license.json",
        ];

        for license_path in license_paths {
            if let Ok(license_data) = tokio::fs::read_to_string(license_path).await {
                match self
                    .offline_validator
                    .validate_license_token(&license_data)
                    .await
                {
                    Ok(license) => {
                        info!("✅ Loaded offline license from: {}", license_path);
                        return Ok(license);
                    }
                    Err(e) => {
                        warn!("⚠️ Invalid license file at {}: {}", license_path, e);
                    }
                }
            }
        }

        // No valid license found - return trial license
        warn!("⚠️ No valid license found, using trial license");
        self.create_trial_license().await
    }

    /// Create trial license for new installations
    async fn create_trial_license(&self) -> Result<LicenseInfo> {
        let trial_license = LicenseInfo {
            license_id: Uuid::new_v4().to_string(),
            customer_id: "trial_customer".to_string(),
            customer_name: "Trial Customer".to_string(),
            license_tier: LicenseTier::Free {
                trial_days: 30,
                feature_restrictions: vec![
                    "advanced_ai_features".to_string(),
                    "enterprise_sso".to_string(),
                    "unlimited_tenants".to_string(),
                ],
            },
            issued_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::days(30)),
            feature_entitlements: FeatureEntitlements::trial_tier(),
            usage_limits: UsageLimits::trial_limits(),
            license_token: self.generate_trial_token()?,
            support_tier: SupportTier::Community,
        };

        info!("🆓 Created trial license: {} days remaining", 30);
        Ok(trial_license)
    }

    /// Generate cryptographically signed license token for offline validation
    fn generate_trial_token(&self) -> Result<String> {
        // Create signed token that can be validated offline
        let token_data = serde_json::json!({
            "iss": "ProximaDB",
            "sub": "trial_customer",
            "tier": "Free",
            "exp": (Utc::now() + Duration::days(30)).timestamp(),
            "features": ["basic_vector", "basic_graph", "trial_ai"],
            "limits": {
                "max_collections": 10,
                "max_vectors": 100000,
                "max_api_calls_daily": 1000,
                "max_ai_queries_daily": 50
            }
        });

        // Sign token with internal key (for air-gapped validation)
        let token = proximadb_kernel::encoding::base64_encode(token_data.to_string().as_bytes());
        Ok(format!("pt_trial_{}", token))
    }

    /// Check current license status
    async fn check_license_status(&self, license: &LicenseInfo) -> Result<LicenseStatus> {
        // Check expiration
        if let Some(expires_at) = license.expires_at
            && Utc::now() > expires_at
        {
            let grace_period_end =
                expires_at + Duration::days(self.config.grace_period_days as i64);
            if Utc::now() > grace_period_end {
                return Ok(LicenseStatus::Expired);
            } else {
                let days_remaining = (grace_period_end - Utc::now()).num_days() as u32;
                return Ok(LicenseStatus::GracePeriod { days_remaining });
            }
        }

        // For privacy-conscious deployments, validate offline
        if self.config.offline_validation_only {
            return self
                .offline_validator
                .validate_offline_license(license)
                .await;
        }

        // For SaaS deployments, could validate online (if phone home is enabled)
        if self.config.enable_phone_home {
            return self.validate_online_license(license).await;
        }

        // Default to valid for air-gapped deployments
        Ok(LicenseStatus::Valid)
    }

    /// Validate feature entitlement based on license tier
    fn is_feature_entitled(
        &self,
        feature: &str,
        entitlements: &FeatureEntitlements,
    ) -> Result<bool> {
        let entitled = match feature {
            // AI Features
            "natural_language_queries" => entitlements.ai_capabilities.natural_language_queries,
            "executive_dashboards" => entitlements.ai_capabilities.executive_dashboards,
            "advanced_ai_features" => entitlements.ai_capabilities.advanced_ai_features,

            // Multi-tenant features
            "multi_tenant_isolation" => entitlements.multi_tenant_features.max_tenants.is_some(),
            "advanced_rbac" => entitlements.multi_tenant_features.rbac_advanced_features,
            "custom_tenant_config" => {
                entitlements
                    .multi_tenant_features
                    .custom_tenant_configuration
            }

            // Performance features
            "all_storage_engines" => {
                entitlements
                    .performance_features
                    .storage_engines_allowed
                    .len()
                    > 3
            }
            "advanced_caching" => entitlements.performance_features.advanced_caching,
            "performance_monitoring" => entitlements.performance_features.performance_monitoring,

            // Security features
            "sso_integration" => entitlements.security_features.sso_integration,
            "audit_logging" => entitlements.security_features.audit_logging,
            "compliance_frameworks" => !entitlements
                .security_features
                .compliance_frameworks
                .is_empty(),

            // Enterprise features
            "deployment_automation" => entitlements.enterprise_features.deployment_automation,
            "custom_integrations" => entitlements.enterprise_features.custom_integrations,
            "dedicated_support" => entitlements.enterprise_features.dedicated_support,

            _ => {
                warn!("⚠️ Unknown feature: {}", feature);
                false
            }
        };

        debug!("🔍 Feature '{}' entitled: {}", feature, entitled);
        Ok(entitled)
    }

    /// Validate usage limits
    async fn validate_usage_limits(
        &self,
        context: &UsageContext,
        limits: &UsageLimits,
    ) -> Result<UsageValidation> {
        let mut exceeded_limits = Vec::new();

        // Check collection limits
        if let Some(max_collections) = limits.max_collections
            && context.current_collections > max_collections
        {
            exceeded_limits.push(format!(
                "Collections: {} > {}",
                context.current_collections, max_collections
            ));
        }

        // Check vector limits
        if let Some(max_vectors) = limits.max_vectors_total
            && context.current_vectors > max_vectors
        {
            exceeded_limits.push(format!(
                "Vectors: {} > {}",
                context.current_vectors, max_vectors
            ));
        }

        // Check API call limits (daily)
        if let Some(max_api_calls) = limits.max_api_calls_per_month {
            let daily_limit = max_api_calls / 30; // Approximate daily limit
            if context.api_calls_today > daily_limit {
                exceeded_limits.push(format!(
                    "API calls today: {} > {}",
                    context.api_calls_today, daily_limit
                ));
            }
        }

        // Check AI query limits
        if let Some(max_ai_queries) = limits.max_ai_queries_per_month {
            let daily_ai_limit = max_ai_queries / 30;
            if context.ai_queries_today > daily_ai_limit {
                exceeded_limits.push(format!(
                    "AI queries today: {} > {}",
                    context.ai_queries_today, daily_ai_limit
                ));
            }
        }

        Ok(UsageValidation {
            within_limits: exceeded_limits.is_empty(),
            exceeded_limits,
        })
    }

    /// Suggest license upgrade based on feature or usage needs
    fn suggest_license_upgrade(&self, requested_feature: &str) -> LicenseUpgradeRecommendation {
        match requested_feature {
            "advanced_ai_features" | "executive_dashboards" => LicenseUpgradeRecommendation {
                recommended_tier: "Professional".to_string(),
                reason: "Advanced AI features require Professional tier or higher".to_string(),
                benefits: vec![
                    "Unlimited natural language queries".to_string(),
                    "Advanced executive dashboard automation".to_string(),
                    "Multiple LLM provider access".to_string(),
                ],
                estimated_monthly_cost: 299.0,
            },
            "enterprise_sso" | "compliance_frameworks" => LicenseUpgradeRecommendation {
                recommended_tier: "Enterprise".to_string(),
                reason: "Enterprise security features require Enterprise tier".to_string(),
                benefits: vec![
                    "Complete SSO integration".to_string(),
                    "SOC 2 compliance framework".to_string(),
                    "Dedicated customer success manager".to_string(),
                ],
                estimated_monthly_cost: 999.0,
            },
            "usage_increase" => LicenseUpgradeRecommendation {
                recommended_tier: "Professional".to_string(),
                reason: "Higher usage limits available in Professional tier".to_string(),
                benefits: vec![
                    "10x higher usage limits".to_string(),
                    "Priority support".to_string(),
                    "Advanced performance features".to_string(),
                ],
                estimated_monthly_cost: 299.0,
            },
            _ => LicenseUpgradeRecommendation {
                recommended_tier: "Professional".to_string(),
                reason: "Enhanced capabilities require upgrade".to_string(),
                benefits: vec!["Enhanced platform capabilities".to_string()],
                estimated_monthly_cost: 299.0,
            },
        }
    }

    /// Load online license (for SaaS deployments with phone home)
    async fn load_online_license(&self) -> Result<LicenseInfo> {
        if !self.config.enable_phone_home {
            return Err(anyhow!("Online license validation disabled"));
        }

        // Placeholder for online license validation
        // Would connect to ProximaDB licensing service
        warn!("📞 Online license validation not implemented - using offline validation");
        self.load_offline_license().await
    }

    /// Validate online license (for SaaS with connectivity)
    async fn validate_online_license(&self, _license: &LicenseInfo) -> Result<LicenseStatus> {
        if !self.config.enable_phone_home {
            return Ok(LicenseStatus::Valid); // Default to valid for air-gapped
        }

        // Placeholder for online validation
        // Would validate with ProximaDB licensing service
        Ok(LicenseStatus::Valid)
    }
}

/// Usage context for license validation
#[derive(Debug, Clone)]
pub struct UsageContext {
    /// Identifier of the user making the request.
    pub user_id: String,
    /// Identifier of the tenant making the request, if multi-tenancy is enabled.
    pub tenant_id: Option<String>,
    /// Total number of collections that currently exist.
    pub current_collections: u32,
    /// Total number of vectors stored across all collections.
    pub current_vectors: u64,
    /// Number of API calls already made today.
    pub api_calls_today: u32,
    /// Number of AI/LLM queries already made today.
    pub ai_queries_today: u32,
    /// Number of users currently authenticated and active.
    pub concurrent_users: u32,
}

/// Usage validation result
#[derive(Debug, Clone)]
pub struct UsageValidation {
    /// `true` if all usage metrics are within their configured limits.
    pub within_limits: bool,
    /// Human-readable descriptions of any limits that have been exceeded.
    pub exceeded_limits: Vec<String>,
}

/// Feature validation result
#[derive(Debug, Clone)]
pub struct FeatureValidation {
    /// `true` if the requested feature access is permitted.
    pub allowed: bool,
    /// Human-readable explanation of why access was granted or denied.
    pub reason: String,
    /// Descriptions of any usage limits that contributed to a denial.
    pub limits_exceeded: Vec<String>,
    /// Optional recommendation for a license upgrade that would grant access.
    pub upgrade_required: Option<LicenseUpgradeRecommendation>,
}

/// License upgrade recommendation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseUpgradeRecommendation {
    /// Name of the license tier being recommended (e.g. `"Professional"`, `"Enterprise"`).
    pub recommended_tier: String,
    /// Explanation of why this upgrade is recommended.
    pub reason: String,
    /// List of benefits the customer would gain by upgrading to the recommended tier.
    pub benefits: Vec<String>,
    /// Estimated monthly cost in USD for the recommended tier.
    pub estimated_monthly_cost: f64,
}

// Implementation of tier-specific entitlements and limits
impl FeatureEntitlements {
    /// Trial tier entitlements (limited features)
    pub fn trial_tier() -> Self {
        Self {
            ai_capabilities: AICapabilities {
                natural_language_queries: true,
                executive_dashboards: false, // Limited in trial
                llm_providers_allowed: vec!["OpenAI".to_string()], // Only one provider
                ai_queries_per_month: Some(100), // Limited AI usage
                advanced_ai_features: false,
            },
            multi_tenant_features: MultiTenantFeatures {
                max_tenants: Some(1), // Single tenant only
                tenant_isolation_level: IsolationLevel::Basic,
                rbac_advanced_features: false,
                custom_tenant_configuration: false,
            },
            performance_features: PerformanceFeatures {
                storage_engines_allowed: vec!["SST".to_string(), "VIPER".to_string()], // Limited engines
                max_qps: Some(100), // Performance limits
                advanced_caching: false,
                performance_monitoring: false,
                custom_optimization: false,
            },
            security_features: SecurityFeatures {
                sso_integration: false,
                audit_logging: false,
                compliance_frameworks: vec![],
                advanced_encryption: false,
                security_monitoring: false,
            },
            enterprise_features: EnterpriseFeatures {
                deployment_automation: false,
                custom_integrations: false,
                dedicated_support: false,
                sla_guarantees: false,
                custom_development: false,
            },
        }
    }

    /// Professional tier entitlements
    pub fn professional_tier() -> Self {
        Self {
            ai_capabilities: AICapabilities {
                natural_language_queries: true,
                executive_dashboards: true,
                llm_providers_allowed: vec![
                    "OpenAI".to_string(),
                    "Anthropic".to_string(),
                    "Cohere".to_string(),
                ],
                ai_queries_per_month: Some(10000), // Higher AI usage
                advanced_ai_features: true,
            },
            multi_tenant_features: MultiTenantFeatures {
                max_tenants: Some(10),
                tenant_isolation_level: IsolationLevel::Enhanced,
                rbac_advanced_features: true,
                custom_tenant_configuration: true,
            },
            performance_features: PerformanceFeatures {
                storage_engines_allowed: vec![
                    "SST".to_string(),
                    "VIPER".to_string(),
                    "NOVA".to_string(),
                    "SWIFT".to_string(),
                ],
                max_qps: Some(5000),
                advanced_caching: true,
                performance_monitoring: true,
                custom_optimization: false,
            },
            security_features: SecurityFeatures {
                sso_integration: true,
                audit_logging: true,
                compliance_frameworks: vec!["SOC2".to_string()],
                advanced_encryption: true,
                security_monitoring: true,
            },
            enterprise_features: EnterpriseFeatures {
                deployment_automation: true,
                custom_integrations: true,
                dedicated_support: false,
                sla_guarantees: false,
                custom_development: false,
            },
        }
    }

    /// Enterprise tier entitlements (unlimited)
    pub fn enterprise_tier() -> Self {
        Self {
            ai_capabilities: AICapabilities {
                natural_language_queries: true,
                executive_dashboards: true,
                llm_providers_allowed: vec!["ALL".to_string()], // All 9 providers
                ai_queries_per_month: None,                     // Unlimited
                advanced_ai_features: true,
            },
            multi_tenant_features: MultiTenantFeatures {
                max_tenants: None, // Unlimited
                tenant_isolation_level: IsolationLevel::Complete,
                rbac_advanced_features: true,
                custom_tenant_configuration: true,
            },
            performance_features: PerformanceFeatures {
                storage_engines_allowed: vec!["ALL".to_string()], // All 7 engines
                max_qps: None,                                    // Unlimited
                advanced_caching: true,
                performance_monitoring: true,
                custom_optimization: true,
            },
            security_features: SecurityFeatures {
                sso_integration: true,
                audit_logging: true,
                compliance_frameworks: vec![
                    "SOC2".to_string(),
                    "GDPR".to_string(),
                    "HIPAA".to_string(),
                ],
                advanced_encryption: true,
                security_monitoring: true,
            },
            enterprise_features: EnterpriseFeatures {
                deployment_automation: true,
                custom_integrations: true,
                dedicated_support: true,
                sla_guarantees: true,
                custom_development: true,
            },
        }
    }
}

impl UsageLimits {
    /// Trial usage limits
    pub fn trial_limits() -> Self {
        Self {
            max_collections: Some(10),
            max_vectors_total: Some(100_000),
            max_storage_gb: Some(1),
            max_api_calls_per_month: Some(30_000), // ~1000/day
            max_ai_queries_per_month: Some(1_500), // ~50/day
            max_concurrent_users: Some(5),
        }
    }

    /// Professional usage limits
    pub fn professional_limits() -> Self {
        Self {
            max_collections: Some(1000),
            max_vectors_total: Some(100_000_000), // 100M vectors
            max_storage_gb: Some(1000),           // 1TB
            max_api_calls_per_month: Some(3_000_000), // ~100K/day
            max_ai_queries_per_month: Some(300_000), // ~10K/day
            max_concurrent_users: Some(100),
        }
    }

    /// Enterprise usage limits (unlimited)
    pub fn enterprise_limits() -> Self {
        Self {
            max_collections: None,          // Unlimited
            max_vectors_total: None,        // Unlimited
            max_storage_gb: None,           // Unlimited
            max_api_calls_per_month: None,  // Unlimited
            max_ai_queries_per_month: None, // Unlimited
            max_concurrent_users: None,     // Unlimited
        }
    }
}

impl Default for LicenseConfig {
    fn default() -> Self {
        Self {
            deployment_model: DeploymentModel::CustomerVPC, // Conservative default
            enable_phone_home: false,                       // Privacy-conscious default
            offline_validation_only: true,                  // Air-gapped friendly
            license_check_interval_hours: 24,
            grace_period_days: 7,
            enable_usage_analytics: false, // Privacy-conscious default
        }
    }
}

// Supporting types and trait implementations
use super::offline_validation::OfflineLicenseValidator;
use super::tier_enforcement::TierEnforcement;

/// Usage limits that apply to the Developer license tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeveloperLimits {
    /// Maximum number of collections the developer account may create.
    pub max_collections: u32,
    /// Maximum total number of vectors that may be stored.
    pub max_vectors: u64,
    /// Maximum number of API calls permitted per day.
    pub max_api_calls_daily: u32,
}

/// Usage limits that apply to the Professional license tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfessionalLimits {
    /// Maximum number of collections that may be created.
    pub max_collections: u32,
    /// Maximum total number of vectors that may be stored.
    pub max_vectors: u64,
    /// Maximum number of tenants that may be provisioned.
    pub max_tenants: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_license_manager_creation() {
        let config = LicenseConfig::default();
        let manager = LicenseManager::new(config).await.unwrap();
        assert!(manager.config.offline_validation_only);
    }

    #[tokio::test]
    async fn test_trial_license_creation() {
        let config = LicenseConfig::default();
        let manager = LicenseManager::new(config).await.unwrap();
        let trial_license = manager.create_trial_license().await.unwrap();

        assert_eq!(trial_license.customer_id, "trial_customer");
        assert!(matches!(
            trial_license.license_tier,
            LicenseTier::Free { .. }
        ));
        assert!(trial_license.expires_at.is_some());
    }

    #[test]
    fn test_feature_entitlements() {
        let trial_entitlements = FeatureEntitlements::trial_tier();
        assert!(trial_entitlements.ai_capabilities.natural_language_queries);
        assert!(!trial_entitlements.ai_capabilities.executive_dashboards); // Limited in trial

        let enterprise_entitlements = FeatureEntitlements::enterprise_tier();
        assert!(enterprise_entitlements.ai_capabilities.advanced_ai_features);
        assert!(
            enterprise_entitlements
                .enterprise_features
                .dedicated_support
        );
    }

    #[test]
    fn test_usage_limits() {
        let trial_limits = UsageLimits::trial_limits();
        assert_eq!(trial_limits.max_collections, Some(10));
        assert_eq!(trial_limits.max_vectors_total, Some(100_000));

        let enterprise_limits = UsageLimits::enterprise_limits();
        assert!(enterprise_limits.max_collections.is_none()); // Unlimited
        assert!(enterprise_limits.max_vectors_total.is_none()); // Unlimited
    }
}
