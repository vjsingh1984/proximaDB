//! Enterprise Trial Management
//!
//! Self-service trial platform for enterprise customer evaluation

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc, Duration};
use uuid::Uuid;
use anyhow::{Result, anyhow};
use tracing::{info, debug};

/// Enterprise trial management for self-service customer evaluation
#[derive(Debug, Clone)]
pub struct EnterpriseTrialManager {
    active_trials: Arc<RwLock<HashMap<String, EnterpriseTrial>>>,
    trial_environments: Arc<TrialEnvironmentManager>,
    customer_analytics: Arc<CustomerEngagementAnalytics>,
    config: TrialConfig,
}

/// Configuration for trial management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialConfig {
    pub trial_duration_days: u32,
    pub max_concurrent_trials: u32,
    pub enable_trial_extensions: bool,
    pub enable_custom_data_loading: bool,
    pub trial_resource_limits: TrialResourceLimits,
}

/// Enterprise trial instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnterpriseTrial {
    pub trial_id: String,
    pub customer_email: String,
    pub company_name: String,
    pub trial_type: TrialType,
    pub status: TrialStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub environment_details: TrialEnvironment,
    pub evaluation_progress: EvaluationProgress,
    pub engagement_metrics: EngagementMetrics,
}

/// Types of enterprise trials
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TrialType {
    AIShowcase,           // Focus on AI and natural language capabilities
    PerformanceTrial,     // Focus on performance and scale
    SecurityEvaluation,   // Focus on enterprise security and compliance
    ComprehensiveEval,    // Full platform evaluation
    CustomPOC,           // Custom proof-of-concept
}

/// Trial status tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrialStatus {
    Provisioning,
    Active,
    Extended,
    Expired,
    Converted,
    Abandoned,
}

/// Trial environment details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialEnvironment {
    pub environment_id: String,
    pub rest_endpoint: String,
    pub grpc_endpoint: String,
    pub dashboard_url: String,
    pub api_key: String,
    pub sample_data_loaded: bool,
    pub ai_features_enabled: bool,
    pub trial_subdomain: String,
}

/// Customer evaluation progress tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationProgress {
    pub milestones_completed: Vec<EvaluationMilestone>,
    pub completion_percentage: f64,
    pub time_spent_minutes: u32,
    pub features_explored: Vec<String>,
    pub pain_points_identified: Vec<String>,
    pub success_criteria_met: Vec<String>,
}

/// Evaluation milestone for customer success tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationMilestone {
    pub milestone_id: String,
    pub name: String,
    pub description: String,
    pub completed_at: Option<DateTime<Utc>>,
    pub value_demonstrated: String,
}

/// Customer engagement metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngagementMetrics {
    pub total_api_calls: u32,
    pub unique_features_used: u32,
    pub ai_queries_executed: u32,
    pub dashboard_views: u32,
    pub documentation_pages_viewed: u32,
    pub support_interactions: u32,
    pub last_activity: DateTime<Utc>,
}

/// Resource limits for trial environments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialResourceLimits {
    pub max_collections: u32,
    pub max_vectors_per_collection: u64,
    pub max_api_calls_per_day: u32,
    pub max_ai_queries_per_day: u32,
    pub storage_limit_gb: u32,
}

impl EnterpriseTrialManager {
    /// Create new enterprise trial manager
    pub async fn new(config: TrialConfig) -> Result<Self> {
        let trial_environments = Arc::new(TrialEnvironmentManager::new().await?);
        let customer_analytics = Arc::new(CustomerEngagementAnalytics::new().await?);

        info!("🚀 Enterprise trial manager initialized with {} day trial duration", config.trial_duration_days);

        Ok(Self {
            active_trials: Arc::new(RwLock::new(HashMap::new())),
            trial_environments,
            customer_analytics,
            config,
        })
    }

    /// Create new enterprise trial for customer
    pub async fn create_enterprise_trial(&self, request: TrialCreationRequest) -> Result<EnterpriseTrial> {
        info!("🎯 Creating enterprise trial for {}: {}", request.company_name, request.customer_email);

        // Step 1: Validate trial request
        self.validate_trial_request(&request).await?;

        // Step 2: Check concurrent trial limits
        let active_trial_count = self.count_active_trials().await;
        if active_trial_count >= self.config.max_concurrent_trials {
            return Err(anyhow!("Maximum concurrent trials reached: {}", self.config.max_concurrent_trials));
        }

        // Step 3: Provision trial environment
        let trial_environment = self.trial_environments.provision_trial_environment(&request).await?;

        // Step 4: Load appropriate sample data
        self.load_trial_sample_data(&trial_environment, &request.trial_type).await?;

        // Step 5: Create trial record
        let trial = EnterpriseTrial {
            trial_id: Uuid::new_v4().to_string(),
            customer_email: request.customer_email.clone(),
            company_name: request.company_name.clone(),
            trial_type: request.trial_type.clone(),
            status: TrialStatus::Active,
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::days(self.config.trial_duration_days as i64),
            environment_details: trial_environment,
            evaluation_progress: EvaluationProgress::new(),
            engagement_metrics: EngagementMetrics::new(),
        };

        // Step 6: Store trial and start monitoring
        let mut trials = self.active_trials.write().await;
        trials.insert(trial.trial_id.clone(), trial.clone());

        // Step 7: Send welcome email and start engagement tracking
        self.send_trial_welcome_email(&trial).await?;
        self.customer_analytics.start_trial_tracking(&trial).await?;

        info!("✅ Enterprise trial created successfully: {} for {} (expires: {})",
              trial.trial_id, trial.company_name, trial.expires_at.format("%Y-%m-%d"));

        Ok(trial)
    }

    /// Load trial sample data based on trial type
    async fn load_trial_sample_data(&self, environment: &TrialEnvironment, trial_type: &TrialType) -> Result<()> {
        info!("📊 Loading sample data for trial type: {:?}", trial_type);

        match trial_type {
            TrialType::AIShowcase => {
                // Load data optimized for AI demonstration
                self.load_ai_showcase_data(environment).await?;
                info!("✅ AI showcase data loaded: business intelligence samples");
            }
            TrialType::PerformanceTrial => {
                // Load large dataset for performance testing
                self.load_performance_test_data(environment).await?;
                info!("✅ Performance test data loaded: 1M+ vectors for scale testing");
            }
            TrialType::SecurityEvaluation => {
                // Load multi-tenant data for security demonstration
                self.load_security_demo_data(environment).await?;
                info!("✅ Security demo data loaded: multi-tenant isolation examples");
            }
            TrialType::ComprehensiveEval => {
                // Load comprehensive dataset covering all features
                self.load_comprehensive_demo_data(environment).await?;
                info!("✅ Comprehensive demo data loaded: full platform showcase");
            }
            TrialType::CustomPOC => {
                // Load customer-specific data if provided
                self.load_custom_poc_data(environment).await?;
                info!("✅ Custom POC data loaded: customer-specific scenarios");
            }
        }

        Ok(())
    }

    /// Track customer engagement and update conversion signals
    pub async fn track_customer_engagement(&self, trial_id: &str, activity: CustomerActivity) -> Result<()> {
        let mut trials = self.active_trials.write().await;

        if let Some(trial) = trials.get_mut(trial_id) {
            // Update engagement metrics
            trial.engagement_metrics.record_activity(&activity);

            // Check for milestone completion
            if let Some(milestone) = self.check_milestone_completion(&activity, &trial.evaluation_progress) {
                let milestone_name = milestone.name.clone();
                trial.evaluation_progress.complete_milestone(milestone);
                info!("🎉 Milestone completed for trial {}: {}", trial_id, milestone_name);
            }

            // Update completion percentage
            trial.evaluation_progress.update_completion_percentage();

            // Check for conversion signals
            if self.should_trigger_sales_outreach(&trial.engagement_metrics) {
                self.trigger_sales_outreach(trial).await?;
            }

            debug!("📊 Updated engagement for trial {}: {:.1}% complete, {} API calls",
                   trial_id, trial.evaluation_progress.completion_percentage, trial.engagement_metrics.total_api_calls);
        }

        Ok(())
    }

    /// Check if sales outreach should be triggered
    fn should_trigger_sales_outreach(&self, metrics: &EngagementMetrics) -> bool {
        // High engagement indicators
        metrics.ai_queries_executed > 20 ||
        metrics.dashboard_views > 10 ||
        metrics.total_api_calls > 100 ||
        metrics.unique_features_used > 5
    }

    /// Check if a milestone should be completed based on customer activity
    fn check_milestone_completion(
        &self,
        activity: &CustomerActivity,
        progress: &EvaluationProgress
    ) -> Option<EvaluationMilestone> {
        // Define milestones based on activity types
        match activity.activity_type {
            ActivityType::TrialStarted => {
                if !progress.milestones_completed.iter().any(|m| m.milestone_id == "onboarding_complete") {
                    Some(EvaluationMilestone {
                        milestone_id: "onboarding_complete".to_string(),
                        name: "Trial Onboarding Complete".to_string(),
                        description: "Customer successfully started their trial".to_string(),
                        completed_at: Some(activity.timestamp),
                        value_demonstrated: "Ease of setup and onboarding".to_string(),
                    })
                } else {
                    None
                }
            },
            ActivityType::DataUploaded => {
                if !progress.milestones_completed.iter().any(|m| m.milestone_id == "data_ingestion") {
                    Some(EvaluationMilestone {
                        milestone_id: "data_ingestion".to_string(),
                        name: "First Data Successfully Ingested".to_string(),
                        description: "Customer ingested their first dataset".to_string(),
                        completed_at: Some(activity.timestamp),
                        value_demonstrated: "Data ingestion capabilities".to_string(),
                    })
                } else {
                    None
                }
            },
            ActivityType::AIQueryExecuted => {
                // Check if they've executed enough queries for search milestone
                let query_count = progress.milestones_completed.iter()
                    .filter(|m| m.milestone_id == "query_milestone")
                    .count();
                if query_count == 0 {
                    Some(EvaluationMilestone {
                        milestone_id: "query_milestone".to_string(),
                        name: "Search Capabilities Demonstrated".to_string(),
                        description: "Customer successfully executed search queries".to_string(),
                        completed_at: Some(activity.timestamp),
                        value_demonstrated: "Search and retrieval performance".to_string(),
                    })
                } else {
                    None
                }
            },
            _ => None, // No milestone for other activity types yet
        }
    }

    /// Trigger sales outreach for high-engagement trial
    async fn trigger_sales_outreach(&self, trial: &EnterpriseTrial) -> Result<()> {
        info!("📞 Triggering sales outreach for high-engagement trial: {} ({})",
              trial.trial_id, trial.company_name);

        // Send internal sales notification
        self.send_sales_notification(trial).await?;

        // Update trial status for sales team visibility
        // (This would integrate with CRM systems)

        Ok(())
    }

    /// Send welcome email to trial customer
    async fn send_trial_welcome_email(&self, trial: &EnterpriseTrial) -> Result<()> {
        let welcome_content = format!(
            "Welcome to your ProximaDB Enterprise Trial!

Dear {},

Your ProximaDB Enterprise trial is now ready:

🚀 Trial Details:
- Trial ID: {}
- Duration: {} days
- Expires: {}

🔗 Access Your Environment:
- REST API: {}
- Dashboard: {}
- API Key: {}

🎯 Getting Started:
1. Explore the AI-powered executive dashboard
2. Try natural language queries like 'Show me top customers by revenue'
3. Test vector similarity search with our sample data
4. Review multi-tenant security and compliance features

📚 Resources:
- Documentation: https://docs.proximadb.com
- API Reference: https://api-docs.proximadb.com
- Support: trial-support@proximadb.com

We're here to help you evaluate ProximaDB's comprehensive AI-powered enterprise platform!

Best regards,
ProximaDB Enterprise Team",
            trial.company_name,
            trial.trial_id,
            self.config.trial_duration_days,
            trial.expires_at.format("%Y-%m-%d"),
            trial.environment_details.rest_endpoint,
            trial.environment_details.dashboard_url,
            trial.environment_details.api_key
        );

        // Send email (placeholder - would integrate with email service)
        self.send_email(&trial.customer_email, "ProximaDB Enterprise Trial Ready", &welcome_content).await?;

        info!("📧 Welcome email sent to: {}", trial.customer_email);
        Ok(())
    }

    /// Send sales notification for internal team
    async fn send_sales_notification(&self, trial: &EnterpriseTrial) -> Result<()> {
        let notification = SalesNotification {
            trial_id: trial.trial_id.clone(),
            company_name: trial.company_name.clone(),
            customer_email: trial.customer_email.clone(),
            engagement_score: self.calculate_engagement_score(&trial.engagement_metrics),
            conversion_probability: self.estimate_conversion_probability(trial).await?,
            recommended_actions: self.generate_sales_recommendations(trial).await?,
            notification_time: Utc::now(),
        };

        // Send to sales team (placeholder - would integrate with CRM/Slack)
        self.notify_sales_team(&notification).await?;

        Ok(())
    }

    // Implementation of data loading methods
    async fn load_ai_showcase_data(&self, environment: &TrialEnvironment) -> Result<()> {
        // Load sample business data for AI demonstrations
        let sample_data = self.create_ai_showcase_dataset().await?;
        self.upload_sample_data(environment, sample_data).await?;
        Ok(())
    }

    async fn load_performance_test_data(&self, environment: &TrialEnvironment) -> Result<()> {
        // Load large dataset for performance demonstrations
        let performance_data = self.create_performance_dataset().await?;
        self.upload_sample_data(environment, performance_data).await?;
        Ok(())
    }

    async fn load_security_demo_data(&self, environment: &TrialEnvironment) -> Result<()> {
        // Load multi-tenant sample data for security demonstrations
        let security_data = self.create_security_dataset().await?;
        self.upload_sample_data(environment, security_data).await?;
        Ok(())
    }

    async fn load_comprehensive_demo_data(&self, environment: &TrialEnvironment) -> Result<()> {
        // Load comprehensive dataset covering all features
        let comprehensive_data = self.create_comprehensive_dataset().await?;
        self.upload_sample_data(environment, comprehensive_data).await?;
        Ok(())
    }

    async fn load_custom_poc_data(&self, environment: &TrialEnvironment) -> Result<()> {
        // Load customer-specific data for custom POCs
        info!("📋 Custom POC data loading - would be customer-specific");
        Ok(())
    }

    // Helper methods
    async fn validate_trial_request(&self, request: &TrialCreationRequest) -> Result<()> {
        if request.customer_email.is_empty() || !request.customer_email.contains('@') {
            return Err(anyhow!("Valid email address required"));
        }

        if request.company_name.is_empty() {
            return Err(anyhow!("Company name required"));
        }

        Ok(())
    }

    async fn count_active_trials(&self) -> u32 {
        let trials = self.active_trials.read().await;
        trials.values().filter(|trial| matches!(trial.status, TrialStatus::Active)).count() as u32
    }

    async fn send_email(&self, _to: &str, _subject: &str, _content: &str) -> Result<()> {
        // Placeholder for email service integration
        Ok(())
    }

    async fn notify_sales_team(&self, _notification: &SalesNotification) -> Result<()> {
        // Placeholder for sales team notification
        Ok(())
    }

    async fn create_ai_showcase_dataset(&self) -> Result<SampleDataset> {
        // Create sample dataset optimized for AI demonstrations
        Ok(SampleDataset::ai_showcase())
    }

    async fn create_performance_dataset(&self) -> Result<SampleDataset> {
        // Create large dataset for performance testing
        Ok(SampleDataset::performance_test())
    }

    async fn create_security_dataset(&self) -> Result<SampleDataset> {
        // Create multi-tenant dataset for security demonstrations
        Ok(SampleDataset::security_demo())
    }

    async fn create_comprehensive_dataset(&self) -> Result<SampleDataset> {
        // Create comprehensive dataset covering all features
        Ok(SampleDataset::comprehensive())
    }

    async fn upload_sample_data(&self, _environment: &TrialEnvironment, _data: SampleDataset) -> Result<()> {
        // Upload sample data to trial environment
        Ok(())
    }

    fn calculate_engagement_score(&self, metrics: &EngagementMetrics) -> f64 {
        let mut score = 0.0;

        // Weight different engagement activities
        score += (metrics.total_api_calls as f64) * 0.01; // API usage
        score += (metrics.ai_queries_executed as f64) * 0.05; // AI engagement
        score += (metrics.dashboard_views as f64) * 0.03; // Dashboard engagement
        score += (metrics.unique_features_used as f64) * 0.1; // Feature exploration

        score.min(1.0) // Cap at 1.0
    }

    async fn estimate_conversion_probability(&self, trial: &EnterpriseTrial) -> Result<f64> {
        let engagement_score = self.calculate_engagement_score(&trial.engagement_metrics);
        let completion_score = trial.evaluation_progress.completion_percentage / 100.0;
        let time_investment = (trial.engagement_metrics.total_api_calls as f64).log10() / 3.0; // Log scale

        // Weighted conversion probability
        let probability = (engagement_score * 0.4) + (completion_score * 0.4) + (time_investment * 0.2);

        Ok(probability.min(1.0))
    }

    async fn generate_sales_recommendations(&self, trial: &EnterpriseTrial) -> Result<Vec<String>> {
        let mut recommendations = Vec::new();

        if trial.engagement_metrics.ai_queries_executed > 10 {
            recommendations.push("Emphasize AI differentiation and business intelligence capabilities".to_string());
        }

        if trial.engagement_metrics.dashboard_views > 5 {
            recommendations.push("Focus on executive dashboard automation and business value".to_string());
        }

        if trial.evaluation_progress.completion_percentage > 50.0 {
            recommendations.push("Schedule closing call within 48 hours - high engagement signals".to_string());
        }

        if recommendations.is_empty() {
            recommendations.push("Provide additional technical resources and use case examples".to_string());
        }

        Ok(recommendations)
    }
}

/// Trial creation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialCreationRequest {
    pub customer_email: String,
    pub company_name: String,
    pub trial_type: TrialType,
    pub industry: Option<String>,
    pub use_case_description: Option<String>,
    pub estimated_data_size: Option<String>,
    pub technical_contact: Option<String>,
}

/// Trial environment manager
#[derive(Debug)]
pub struct TrialEnvironmentManager {
    environment_templates: HashMap<TrialType, EnvironmentTemplate>,
}

impl TrialEnvironmentManager {
    pub async fn new() -> Result<Self> {
        let mut templates = HashMap::new();

        // AI showcase environment template
        templates.insert(TrialType::AIShowcase, EnvironmentTemplate {
            cpu_allocation: 2,
            memory_gb: 8,
            storage_gb: 50,
            ai_providers_enabled: vec!["OpenAI".to_string(), "Anthropic".to_string()],
            sample_data_size: 10000,
        });

        Ok(Self { environment_templates: templates })
    }

    pub async fn provision_trial_environment(&self, request: &TrialCreationRequest) -> Result<TrialEnvironment> {
        let environment_id = Uuid::new_v4().to_string();
        let subdomain = format!("trial-{}", &environment_id[..8]);

        // Create trial environment (would integrate with deployment automation)
        let environment = TrialEnvironment {
            environment_id: environment_id.clone(),
            rest_endpoint: format!("https://{}.proximadb.com", subdomain),
            grpc_endpoint: format!("grpc://{}.proximadb.com:5679", subdomain),
            dashboard_url: format!("https://{}.proximadb.com/dashboard", subdomain),
            api_key: format!("trial_{}", Uuid::new_v4()),
            sample_data_loaded: false,
            ai_features_enabled: true,
            trial_subdomain: subdomain,
        };

        info!("🌐 Trial environment provisioned: {}", environment.trial_subdomain);
        Ok(environment)
    }
}

/// Environment template for different trial types
#[derive(Debug, Clone)]
pub struct EnvironmentTemplate {
    pub cpu_allocation: u32,
    pub memory_gb: u32,
    pub storage_gb: u32,
    pub ai_providers_enabled: Vec<String>,
    pub sample_data_size: usize,
}

/// Customer engagement analytics
#[derive(Debug)]
pub struct CustomerEngagementAnalytics {
    engagement_data: Arc<RwLock<HashMap<String, Vec<CustomerActivity>>>>,
}

impl CustomerEngagementAnalytics {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            engagement_data: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn start_trial_tracking(&self, trial: &EnterpriseTrial) -> Result<()> {
        info!("📈 Starting engagement tracking for trial: {}", trial.trial_id);
        Ok(())
    }
}

/// Customer activity tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerActivity {
    pub activity_type: ActivityType,
    pub timestamp: DateTime<Utc>,
    pub duration_minutes: Option<u32>,
    pub details: HashMap<String, String>,
}

/// Types of customer activities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActivityType {
    TrialStarted,
    DashboardViewed,
    AIQueryExecuted,
    PerformanceTestRun,
    DataUploaded,
    IntegrationTested,
    DocumentationViewed,
    SupportContacted,
    TrialExtended,
    ConversionDiscussion,
}

/// Sample dataset for trials
#[derive(Debug, Clone)]
pub struct SampleDataset {
    pub dataset_type: String,
    pub vector_count: usize,
    pub collections: Vec<String>,
    pub metadata_fields: Vec<String>,
}

impl SampleDataset {
    pub fn ai_showcase() -> Self {
        Self {
            dataset_type: "AI Showcase".to_string(),
            vector_count: 10000,
            collections: vec!["business_data".to_string(), "customer_analytics".to_string()],
            metadata_fields: vec!["category".to_string(), "revenue".to_string(), "region".to_string()],
        }
    }

    pub fn performance_test() -> Self {
        Self {
            dataset_type: "Performance Test".to_string(),
            vector_count: 1000000,
            collections: vec!["large_dataset".to_string(), "scale_test".to_string()],
            metadata_fields: vec!["id".to_string(), "timestamp".to_string()],
        }
    }

    pub fn security_demo() -> Self {
        Self {
            dataset_type: "Security Demo".to_string(),
            vector_count: 50000,
            collections: vec!["tenant_a_data".to_string(), "tenant_b_data".to_string()],
            metadata_fields: vec!["tenant_id".to_string(), "classification".to_string()],
        }
    }

    pub fn comprehensive() -> Self {
        Self {
            dataset_type: "Comprehensive".to_string(),
            vector_count: 100000,
            collections: vec!["products".to_string(), "customers".to_string(), "transactions".to_string()],
            metadata_fields: vec!["category".to_string(), "value".to_string(), "date".to_string()],
        }
    }
}

/// Sales notification for internal team
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalesNotification {
    pub trial_id: String,
    pub company_name: String,
    pub customer_email: String,
    pub engagement_score: f64,
    pub conversion_probability: f64,
    pub recommended_actions: Vec<String>,
    pub notification_time: DateTime<Utc>,
}

// Default implementations
impl EvaluationProgress {
    pub fn new() -> Self {
        Self {
            milestones_completed: vec![],
            completion_percentage: 0.0,
            time_spent_minutes: 0,
            features_explored: vec![],
            pain_points_identified: vec![],
            success_criteria_met: vec![],
        }
    }

    pub fn complete_milestone(&mut self, milestone: EvaluationMilestone) {
        self.milestones_completed.push(milestone);
        self.update_completion_percentage();
    }

    pub fn update_completion_percentage(&mut self) {
        // Calculate completion based on milestones and feature usage
        let milestone_score = self.milestones_completed.len() as f64 * 20.0; // 20% per milestone
        let feature_score = self.features_explored.len() as f64 * 10.0; // 10% per feature
        self.completion_percentage = (milestone_score + feature_score).min(100.0);
    }
}

impl EngagementMetrics {
    pub fn new() -> Self {
        Self {
            total_api_calls: 0,
            unique_features_used: 0,
            ai_queries_executed: 0,
            dashboard_views: 0,
            documentation_pages_viewed: 0,
            support_interactions: 0,
            last_activity: Utc::now(),
        }
    }

    pub fn record_activity(&mut self, activity: &CustomerActivity) {
        self.last_activity = activity.timestamp;

        match activity.activity_type {
            ActivityType::AIQueryExecuted => self.ai_queries_executed += 1,
            ActivityType::DashboardViewed => self.dashboard_views += 1,
            ActivityType::DocumentationViewed => self.documentation_pages_viewed += 1,
            ActivityType::SupportContacted => self.support_interactions += 1,
            _ => {}
        }

        self.total_api_calls += 1;
    }
}

impl Default for TrialConfig {
    fn default() -> Self {
        Self {
            trial_duration_days: 30,
            max_concurrent_trials: 100,
            enable_trial_extensions: true,
            enable_custom_data_loading: true,
            trial_resource_limits: TrialResourceLimits {
                max_collections: 10,
                max_vectors_per_collection: 100000,
                max_api_calls_per_day: 10000,
                max_ai_queries_per_day: 500,
                storage_limit_gb: 10,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_trial_manager_creation() {
        let config = TrialConfig::default();
        let manager = EnterpriseTrialManager::new(config).await.unwrap();
        assert_eq!(manager.config.trial_duration_days, 30);
    }

    #[tokio::test]
    async fn test_trial_creation_request_validation() {
        let manager = create_test_trial_manager().await;

        let valid_request = TrialCreationRequest {
            customer_email: "test@company.com".to_string(),
            company_name: "Test Company".to_string(),
            trial_type: TrialType::AIShowcase,
            industry: Some("Technology".to_string()),
            use_case_description: Some("AI-powered analytics".to_string()),
            estimated_data_size: Some("100K vectors".to_string()),
            technical_contact: Some("tech@company.com".to_string()),
        };

        assert!(manager.validate_trial_request(&valid_request).await.is_ok());

        let invalid_request = TrialCreationRequest {
            customer_email: "invalid-email".to_string(), // Invalid email
            company_name: "".to_string(), // Empty company name
            trial_type: TrialType::AIShowcase,
            industry: None,
            use_case_description: None,
            estimated_data_size: None,
            technical_contact: None,
        };

        assert!(manager.validate_trial_request(&invalid_request).await.is_err());
    }

    async fn create_test_trial_manager() -> EnterpriseTrialManager {
        let config = TrialConfig::default();
        EnterpriseTrialManager::new(config).await.unwrap()
    }
}