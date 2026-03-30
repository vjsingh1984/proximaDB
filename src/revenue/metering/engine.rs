//! Usage Metering Engine
//!
//! Real-time usage tracking and cost calculation for enterprise billing

use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Enterprise usage metering engine for real-time billing
pub struct UsageMeteringEngine {
    /// Real-time usage event storage
    usage_events: Arc<RwLock<HashMap<String, Vec<UsageEvent>>>>,
    /// Aggregated usage metrics per tenant
    usage_aggregates: Arc<RwLock<HashMap<String, UsageAggregate>>>,
    /// Pricing configuration
    pricing_config: Arc<PricingConfig>,
    /// Billing integration
    billing_integration: Option<Arc<dyn BillingProvider + Send + Sync>>,
    /// Configuration
    #[allow(dead_code)]
    config: MeteringConfig,
}

/// Configuration for usage metering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeteringConfig {
    pub enable_real_time_metering: bool,
    pub billing_threshold_usd: f64,
    pub billing_frequency_days: u32,
    pub enable_usage_analytics: bool,
    pub enable_cost_optimization: bool,
}

/// Usage event for enterprise metering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub event_id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub event_type: UsageEventType,
    pub timestamp: DateTime<Utc>,
    pub resource_consumed: ResourceConsumption,
    pub cost_impact: Option<f64>,
    pub metadata: HashMap<String, String>,
}

/// Types of billable usage events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UsageEventType {
    VectorSearch { k: usize, dimensions: usize },
    VectorInsertion { count: usize, dimensions: usize },
    CollectionCreation { estimated_size: u64 },
    AIQuery { provider: String, tokens: u32 },
    DataStorage { bytes: u64 },
    ComputeTime { milliseconds: u64 },
    DataTransfer { bytes: u64 },
    ExecutiveDashboard { insights_generated: u32 },
    NaturalLanguageQuery { complexity_score: u32 },
}

/// Resource consumption metrics for precise billing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceConsumption {
    pub cpu_milliseconds: u64,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub network_bytes: u64,
    pub ai_tokens: u32,
    pub custom_metrics: HashMap<String, f64>,
}

/// Aggregated usage for billing periods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageAggregate {
    pub tenant_id: String,
    pub billing_period_start: DateTime<Utc>,
    pub billing_period_end: DateTime<Utc>,
    pub total_searches: u64,
    pub total_insertions: u64,
    pub total_ai_queries: u64,
    pub total_storage_bytes: u64,
    pub total_compute_ms: u64,
    pub total_cost: f64,
    pub usage_by_day: HashMap<String, DailyUsage>,
    pub peak_usage_metrics: PeakUsageMetrics,
}

/// Daily usage breakdown for analytics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyUsage {
    pub date: String,
    pub searches: u64,
    pub insertions: u64,
    pub ai_queries: u64,
    pub executive_dashboards: u32,
    pub cost: f64,
    pub peak_qps: f64,
}

/// Peak usage metrics for capacity planning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeakUsageMetrics {
    pub peak_qps: f64,
    pub peak_concurrent_users: u32,
    pub peak_storage_gb: f64,
    pub peak_ai_requests_per_hour: u32,
}

/// Pricing configuration for enterprise billing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    pub search_cost_per_1k: f64,            // Cost per 1000 vector searches
    pub insertion_cost_per_1k: f64,         // Cost per 1000 vector insertions
    pub ai_cost_per_1k_tokens: f64,         // Cost per 1000 AI tokens
    pub storage_cost_per_gb: f64,           // Monthly cost per GB storage
    pub compute_cost_per_hour: f64,         // Cost per compute hour
    pub transfer_cost_per_gb: f64,          // Cost per GB data transfer
    pub collection_creation_base_cost: f64, // Base cost for collection creation
    pub executive_dashboard_cost: f64,      // Cost per executive dashboard generation
    pub natural_language_query_cost: f64,   // Cost per natural language query
    pub billing_threshold: f64,             // Trigger billing at this cost amount
    pub billing_frequency_days: u32,        // Bill every N days
    pub enterprise_discount_tiers: Vec<DiscountTier>,
}

/// Discount tiers for enterprise customers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscountTier {
    pub name: String,
    pub min_monthly_spend: f64,
    pub discount_percentage: f64,
    pub additional_benefits: Vec<String>,
}

impl UsageMeteringEngine {
    /// Create new usage metering engine
    pub async fn new(pricing_config: PricingConfig, config: MeteringConfig) -> Result<Self> {
        info!("💰 Initializing enterprise usage metering engine");

        Ok(Self {
            usage_events: Arc::new(RwLock::new(HashMap::new())),
            usage_aggregates: Arc::new(RwLock::new(HashMap::new())),
            pricing_config: Arc::new(pricing_config),
            billing_integration: None,
            config,
        })
    }

    /// Record billable usage event with real-time cost calculation
    pub async fn record_usage_event(&self, event: UsageEvent) -> Result<()> {
        debug!(
            "💰 Recording usage event: {:?} for tenant {}",
            event.event_type, event.tenant_id
        );

        // Calculate cost impact in real-time
        let cost = self.calculate_event_cost(&event).await?;
        let mut enriched_event = event;
        enriched_event.cost_impact = Some(cost);

        // Store event for detailed analytics
        let mut events = self.usage_events.write().await;
        events
            .entry(enriched_event.tenant_id.clone())
            .or_default()
            .push(enriched_event.clone());

        // Update real-time aggregates
        self.update_usage_aggregates(&enriched_event).await?;

        // Check billing threshold
        self.check_billing_threshold(&enriched_event.tenant_id)
            .await?;

        // Update customer success metrics
        self.update_customer_success_metrics(&enriched_event)
            .await?;

        info!(
            "✅ Usage event recorded: ${:.4} cost for tenant {} ({})",
            cost, enriched_event.tenant_id, enriched_event.event_type
        );

        Ok(())
    }

    /// Calculate precise cost for specific usage event
    async fn calculate_event_cost(&self, event: &UsageEvent) -> Result<f64> {
        let pricing = &self.pricing_config;

        let cost = match &event.event_type {
            UsageEventType::VectorSearch { k, dimensions } => {
                pricing.search_cost_per_1k
                    * (*k as f64 / 1000.0)
                    * pricing.dimension_multiplier(*dimensions)
            }
            UsageEventType::VectorInsertion { count, dimensions } => {
                pricing.insertion_cost_per_1k
                    * (*count as f64 / 1000.0)
                    * pricing.dimension_multiplier(*dimensions)
            }
            UsageEventType::CollectionCreation { estimated_size } => {
                pricing.collection_creation_base_cost
                    + (pricing.storage_cost_per_gb * (*estimated_size as f64 / 1_073_741_824.0))
            }
            UsageEventType::AIQuery {
                provider: _,
                tokens,
            } => pricing.ai_cost_per_1k_tokens * (*tokens as f64 / 1000.0),
            UsageEventType::DataStorage { bytes } => {
                pricing.storage_cost_per_gb * (*bytes as f64 / 1_073_741_824.0)
            }
            UsageEventType::ComputeTime { milliseconds } => {
                pricing.compute_cost_per_hour * (*milliseconds as f64 / 3_600_000.0)
            }
            UsageEventType::DataTransfer { bytes } => {
                pricing.transfer_cost_per_gb * (*bytes as f64 / 1_073_741_824.0)
            }
            UsageEventType::ExecutiveDashboard { insights_generated } => {
                pricing.executive_dashboard_cost * (*insights_generated as f64)
            }
            UsageEventType::NaturalLanguageQuery { complexity_score } => {
                pricing.natural_language_query_cost * (*complexity_score as f64 / 100.0)
            }
        };

        Ok(cost)
    }

    /// Update usage aggregates for billing
    async fn update_usage_aggregates(&self, event: &UsageEvent) -> Result<()> {
        let mut aggregates = self.usage_aggregates.write().await;
        let tenant_aggregate = aggregates
            .entry(event.tenant_id.clone())
            .or_insert_with(|| UsageAggregate::new_for_tenant(&event.tenant_id));

        // Update aggregate counters based on event type
        match &event.event_type {
            UsageEventType::VectorSearch { .. } => {
                tenant_aggregate.total_searches += 1;
                self.update_peak_qps(tenant_aggregate, event).await;
            }
            UsageEventType::VectorInsertion { count, .. } => {
                tenant_aggregate.total_insertions += *count as u64;
            }
            UsageEventType::AIQuery { .. } => {
                tenant_aggregate.total_ai_queries += 1;
                self.update_ai_usage_metrics(tenant_aggregate, event).await;
            }
            UsageEventType::DataStorage { bytes } => {
                tenant_aggregate.total_storage_bytes =
                    tenant_aggregate.total_storage_bytes.max(*bytes);
            }
            UsageEventType::ComputeTime { milliseconds } => {
                tenant_aggregate.total_compute_ms += *milliseconds;
            }
            UsageEventType::ExecutiveDashboard { insights_generated } => {
                self.update_executive_dashboard_usage(tenant_aggregate, *insights_generated)
                    .await;
            }
            _ => {}
        }

        // Update total cost
        tenant_aggregate.total_cost += event.cost_impact.unwrap_or(0.0);

        // Update daily usage tracking
        let date_key = event.timestamp.format("%Y-%m-%d").to_string();
        let daily_usage = tenant_aggregate
            .usage_by_day
            .entry(date_key.clone())
            .or_insert_with(|| DailyUsage::new(&date_key));

        self.update_daily_usage(daily_usage, event).await;

        Ok(())
    }

    /// Update daily usage metrics
    async fn update_daily_usage(&self, daily_usage: &mut DailyUsage, event: &UsageEvent) {
        match &event.event_type {
            UsageEventType::VectorSearch { .. } => {
                daily_usage.searches += 1;
                // Update peak QPS calculation
                let current_hour_searches = daily_usage.searches; // Simplified
                daily_usage.peak_qps = daily_usage.peak_qps.max(current_hour_searches as f64);
            }
            UsageEventType::VectorInsertion { count, .. } => {
                daily_usage.insertions += *count as u64;
            }
            UsageEventType::AIQuery { .. } => {
                daily_usage.ai_queries += 1;
            }
            UsageEventType::ExecutiveDashboard { insights_generated } => {
                daily_usage.executive_dashboards += *insights_generated;
            }
            _ => {}
        }

        daily_usage.cost += event.cost_impact.unwrap_or(0.0);
    }

    /// Check if billing threshold is reached and trigger billing
    async fn check_billing_threshold(&self, tenant_id: &str) -> Result<()> {
        let aggregates = self.usage_aggregates.read().await;

        if let Some(aggregate) = aggregates.get(tenant_id) {
            let should_bill = aggregate.total_cost > self.pricing_config.billing_threshold
                || (Utc::now() - aggregate.billing_period_start).num_days()
                    >= self.pricing_config.billing_frequency_days as i64;

            if should_bill {
                info!(
                    "💳 Billing threshold reached for tenant {}: ${:.2}",
                    tenant_id, aggregate.total_cost
                );

                // Trigger billing process
                if let Some(ref billing_provider) = self.billing_integration {
                    billing_provider
                        .process_billing(tenant_id, aggregate)
                        .await?;
                } else {
                    warn!(
                        "⚠️ Billing threshold reached but no billing provider configured for tenant: {}",
                        tenant_id
                    );
                }
            }
        }

        Ok(())
    }

    /// Get usage summary for customer dashboard
    pub async fn get_usage_summary(
        &self,
        tenant_id: &str,
        period: BillingPeriod,
    ) -> Result<UsageSummary> {
        let aggregates = self.usage_aggregates.read().await;

        if let Some(aggregate) = aggregates.get(tenant_id) {
            let usage_breakdown = self.calculate_usage_breakdown(aggregate).await?;
            let trending = self.calculate_usage_trends(aggregate).await?;
            let recommendations = self
                .generate_cost_optimization_recommendations(aggregate)
                .await?;

            Ok(UsageSummary {
                tenant_id: tenant_id.to_string(),
                period: period.clone(),
                total_cost: aggregate.total_cost,
                usage_breakdown,
                trending,
                recommendations,
                peak_metrics: aggregate.peak_usage_metrics.clone(),
            })
        } else {
            Err(anyhow!("No usage data found for tenant: {}", tenant_id))
        }
    }

    /// Generate cost optimization recommendations
    async fn generate_cost_optimization_recommendations(
        &self,
        aggregate: &UsageAggregate,
    ) -> Result<Vec<CostOptimizationRecommendation>> {
        let mut recommendations = Vec::new();

        // High storage cost optimization
        if aggregate.total_storage_bytes > 100 * 1024 * 1024 * 1024 {
            // >100GB
            recommendations.push(CostOptimizationRecommendation {
                recommendation_type: RecommendationType::StorageOptimization,
                title: "Storage Cost Optimization".to_string(),
                description: "High storage usage detected - consider data archiving or compression"
                    .to_string(),
                estimated_savings_usd: aggregate.total_cost * 0.20, // 20% potential savings
                implementation_effort: ImplementationEffort::Medium,
                priority: RecommendationPriority::High,
            });
        }

        // High AI usage optimization
        if aggregate.total_ai_queries > 10000 {
            recommendations.push(CostOptimizationRecommendation {
                recommendation_type: RecommendationType::AIOptimization,
                title: "AI Usage Optimization".to_string(),
                description: "High AI query volume - consider caching and query optimization"
                    .to_string(),
                estimated_savings_usd: aggregate.total_cost * 0.15, // 15% potential savings
                implementation_effort: ImplementationEffort::Low,
                priority: RecommendationPriority::Medium,
            });
        }

        // Plan upgrade recommendation
        if aggregate.total_cost > 5000.0 {
            recommendations.push(CostOptimizationRecommendation {
                recommendation_type: RecommendationType::PlanUpgrade,
                title: "Enterprise Plan Upgrade".to_string(),
                description: "Usage volume qualifies for enterprise discount tier".to_string(),
                estimated_savings_usd: aggregate.total_cost * 0.25, // 25% enterprise discount
                implementation_effort: ImplementationEffort::Low,
                priority: RecommendationPriority::High,
            });
        }

        Ok(recommendations)
    }

    /// Update customer success metrics based on usage
    async fn update_customer_success_metrics(&self, event: &UsageEvent) -> Result<()> {
        // Track usage patterns for customer success
        match &event.event_type {
            UsageEventType::ExecutiveDashboard { insights_generated } => {
                info!(
                    "📊 Executive engagement: {} insights generated for tenant {}",
                    insights_generated, event.tenant_id
                );
                // High executive dashboard usage indicates strong engagement
            }
            UsageEventType::AIQuery { tokens, .. } => {
                debug!(
                    "🤖 AI engagement: {} tokens used by tenant {}",
                    tokens, event.tenant_id
                );
                // AI usage indicates advanced feature adoption
            }
            _ => {}
        }

        Ok(())
    }

    // Helper methods for usage calculations
    async fn update_peak_qps(&self, aggregate: &mut UsageAggregate, _event: &UsageEvent) {
        // Simplified peak QPS calculation
        let current_qps = aggregate.total_searches as f64
            / (Utc::now() - aggregate.billing_period_start)
                .num_minutes()
                .max(1) as f64
            * 60.0;
        aggregate.peak_usage_metrics.peak_qps =
            aggregate.peak_usage_metrics.peak_qps.max(current_qps);
    }

    async fn update_ai_usage_metrics(&self, aggregate: &mut UsageAggregate, _event: &UsageEvent) {
        // Track AI usage patterns for customer success
        let ai_requests_per_hour = aggregate.total_ai_queries as f32
            / (Utc::now() - aggregate.billing_period_start)
                .num_hours()
                .max(1) as f32;
        aggregate.peak_usage_metrics.peak_ai_requests_per_hour = aggregate
            .peak_usage_metrics
            .peak_ai_requests_per_hour
            .max(ai_requests_per_hour as u32);
    }

    async fn update_executive_dashboard_usage(
        &self,
        _aggregate: &mut UsageAggregate,
        insights_generated: u32,
    ) {
        // Track executive dashboard usage for customer success
        info!(
            "📈 Executive dashboard usage: {} insights generated",
            insights_generated
        );
    }

    async fn calculate_usage_breakdown(
        &self,
        aggregate: &UsageAggregate,
    ) -> Result<UsageBreakdown> {
        let total_cost = aggregate.total_cost;

        // Calculate cost breakdown by category
        let storage_cost_percentage = if total_cost > 0.0 {
            (aggregate.total_storage_bytes as f64 * self.pricing_config.storage_cost_per_gb
                / 1_073_741_824.0)
                / total_cost
                * 100.0
        } else {
            0.0
        };

        let search_cost_percentage = if total_cost > 0.0 {
            (aggregate.total_searches as f64 * self.pricing_config.search_cost_per_1k / 1000.0)
                / total_cost
                * 100.0
        } else {
            0.0
        };

        let ai_cost_percentage = 100.0 - storage_cost_percentage - search_cost_percentage;

        Ok(UsageBreakdown {
            storage_cost_percentage,
            search_cost_percentage,
            ai_cost_percentage,
            top_cost_drivers: vec![
                "Vector searches".to_string(),
                "AI queries".to_string(),
                "Data storage".to_string(),
            ],
        })
    }

    async fn calculate_usage_trends(&self, aggregate: &UsageAggregate) -> Result<UsageTrends> {
        // Calculate week-over-week trends
        let current_week_cost = aggregate.total_cost; // Simplified
        let growth_rate = 15.0; // Placeholder calculation

        Ok(UsageTrends {
            week_over_week_growth: growth_rate,
            cost_trend: if growth_rate > 0.0 {
                TrendDirection::Increasing
            } else {
                TrendDirection::Decreasing
            },
            usage_pattern: UsagePattern::BusinessHours, // Simplified
            forecast_next_month: current_week_cost * 4.0 * (1.0 + growth_rate / 100.0),
        })
    }
}

impl PricingConfig {
    /// Calculate dimension-based pricing multiplier
    pub fn dimension_multiplier(&self, dimensions: usize) -> f64 {
        match dimensions {
            0..=128 => 1.0,
            129..=384 => 1.2,
            385..=768 => 1.5,
            769..=1536 => 2.0,
            _ => 2.5, // Very high dimensional vectors cost more to process
        }
    }

    /// Get enterprise pricing configuration
    pub fn enterprise_default() -> Self {
        Self {
            search_cost_per_1k: 0.001,           // $0.001 per 1000 searches
            insertion_cost_per_1k: 0.002,        // $0.002 per 1000 insertions
            ai_cost_per_1k_tokens: 0.02,         // $0.02 per 1000 AI tokens
            storage_cost_per_gb: 0.10,           // $0.10 per GB per month
            compute_cost_per_hour: 0.05,         // $0.05 per compute hour
            transfer_cost_per_gb: 0.01,          // $0.01 per GB transfer
            collection_creation_base_cost: 0.50, // $0.50 per collection
            executive_dashboard_cost: 0.25,      // $0.25 per executive dashboard
            natural_language_query_cost: 0.05,   // $0.05 per NL query
            billing_threshold: 100.0,            // Bill at $100
            billing_frequency_days: 30,          // Monthly billing
            enterprise_discount_tiers: vec![
                DiscountTier {
                    name: "Growth".to_string(),
                    min_monthly_spend: 1000.0,
                    discount_percentage: 10.0,
                    additional_benefits: vec!["Priority support".to_string()],
                },
                DiscountTier {
                    name: "Scale".to_string(),
                    min_monthly_spend: 10000.0,
                    discount_percentage: 20.0,
                    additional_benefits: vec![
                        "Dedicated CSM".to_string(),
                        "Custom SLA".to_string(),
                    ],
                },
                DiscountTier {
                    name: "Enterprise".to_string(),
                    min_monthly_spend: 50000.0,
                    discount_percentage: 30.0,
                    additional_benefits: vec![
                        "24/7 support".to_string(),
                        "Custom deployment".to_string(),
                    ],
                },
            ],
        }
    }
}

impl UsageAggregate {
    /// Create new usage aggregate for tenant
    pub fn new_for_tenant(tenant_id: &str) -> Self {
        Self {
            tenant_id: tenant_id.to_string(),
            billing_period_start: Utc::now(),
            billing_period_end: Utc::now() + Duration::days(30),
            total_searches: 0,
            total_insertions: 0,
            total_ai_queries: 0,
            total_storage_bytes: 0,
            total_compute_ms: 0,
            total_cost: 0.0,
            usage_by_day: HashMap::new(),
            peak_usage_metrics: PeakUsageMetrics {
                peak_qps: 0.0,
                peak_concurrent_users: 0,
                peak_storage_gb: 0.0,
                peak_ai_requests_per_hour: 0,
            },
        }
    }
}

impl DailyUsage {
    pub fn new(date: &str) -> Self {
        Self {
            date: date.to_string(),
            searches: 0,
            insertions: 0,
            ai_queries: 0,
            executive_dashboards: 0,
            cost: 0.0,
            peak_qps: 0.0,
        }
    }
}

impl std::fmt::Display for UsageEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsageEventType::VectorSearch { k, dimensions } => {
                write!(f, "VectorSearch(k={}, dim={})", k, dimensions)
            }
            UsageEventType::VectorInsertion { count, dimensions } => {
                write!(f, "VectorInsertion(count={}, dim={})", count, dimensions)
            }
            UsageEventType::CollectionCreation { estimated_size } => {
                write!(f, "CollectionCreation(size={})", estimated_size)
            }
            UsageEventType::AIQuery { provider, tokens } => {
                write!(f, "AIQuery(provider={}, tokens={})", provider, tokens)
            }
            UsageEventType::ExecutiveDashboard { insights_generated } => {
                write!(f, "ExecutiveDashboard(insights={})", insights_generated)
            }
            UsageEventType::NaturalLanguageQuery { complexity_score } => {
                write!(f, "NaturalLanguageQuery(complexity={})", complexity_score)
            }
            _ => write!(f, "{:?}", self),
        }
    }
}

/// Trait for billing provider integration
#[async_trait::async_trait]
pub trait BillingProvider: Send + Sync {
    async fn process_billing(&self, tenant_id: &str, usage: &UsageAggregate) -> Result<()>;
}

/// Billing period options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BillingPeriod {
    Daily,
    Weekly,
    Monthly,
    Quarterly,
    Annual,
    Custom {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
}

/// Usage summary for customer dashboards
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub tenant_id: String,
    pub period: BillingPeriod,
    pub total_cost: f64,
    pub usage_breakdown: UsageBreakdown,
    pub trending: UsageTrends,
    pub recommendations: Vec<CostOptimizationRecommendation>,
    pub peak_metrics: PeakUsageMetrics,
}

/// Cost breakdown by category
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageBreakdown {
    pub storage_cost_percentage: f64,
    pub search_cost_percentage: f64,
    pub ai_cost_percentage: f64,
    pub top_cost_drivers: Vec<String>,
}

/// Usage trends for forecasting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageTrends {
    pub week_over_week_growth: f64,
    pub cost_trend: TrendDirection,
    pub usage_pattern: UsagePattern,
    pub forecast_next_month: f64,
}

/// Trend directions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrendDirection {
    Increasing,
    Decreasing,
    Stable,
}

/// Usage patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UsagePattern {
    BusinessHours,
    AlwaysOn,
    Bursty,
    Weekend,
}

/// Cost optimization recommendations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostOptimizationRecommendation {
    pub recommendation_type: RecommendationType,
    pub title: String,
    pub description: String,
    pub estimated_savings_usd: f64,
    pub implementation_effort: ImplementationEffort,
    pub priority: RecommendationPriority,
}

/// Types of cost optimization recommendations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecommendationType {
    StorageOptimization,
    AIOptimization,
    PlanUpgrade,
    CacheOptimization,
    QueryOptimization,
}

/// Implementation effort levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImplementationEffort {
    Low,
    Medium,
    High,
}

/// Recommendation priority levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecommendationPriority {
    Low,
    Medium,
    High,
    Critical,
}

impl Default for MeteringConfig {
    fn default() -> Self {
        Self {
            enable_real_time_metering: true,
            billing_threshold_usd: 100.0,
            billing_frequency_days: 30,
            enable_usage_analytics: true,
            enable_cost_optimization: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_usage_metering_engine_creation() {
        let pricing_config = PricingConfig::enterprise_default();
        let metering_config = MeteringConfig::default();

        let engine = UsageMeteringEngine::new(pricing_config, metering_config)
            .await
            .unwrap();
        assert!(engine.config.enable_real_time_metering);
    }

    #[tokio::test]
    async fn test_usage_event_cost_calculation() {
        let pricing_config = PricingConfig::enterprise_default();
        let metering_config = MeteringConfig::default();
        let engine = UsageMeteringEngine::new(pricing_config, metering_config)
            .await
            .unwrap();

        let search_event = UsageEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            tenant_id: "test_tenant".to_string(),
            user_id: "test_user".to_string(),
            event_type: UsageEventType::VectorSearch {
                k: 10,
                dimensions: 512,
            },
            timestamp: Utc::now(),
            resource_consumed: ResourceConsumption {
                cpu_milliseconds: 100,
                memory_bytes: 1024 * 1024,
                storage_bytes: 0,
                network_bytes: 1024,
                ai_tokens: 0,
                custom_metrics: HashMap::new(),
            },
            cost_impact: None,
            metadata: HashMap::new(),
        };

        let cost = engine.calculate_event_cost(&search_event).await.unwrap();
        assert!(cost > 0.0);
        assert!(cost < 1.0); // Should be small cost for single search
    }

    #[tokio::test]
    async fn test_cost_optimization_recommendations() {
        let pricing_config = PricingConfig::enterprise_default();
        let metering_config = MeteringConfig::default();
        let engine = UsageMeteringEngine::new(pricing_config, metering_config)
            .await
            .unwrap();

        let high_usage_aggregate = UsageAggregate {
            tenant_id: "high_usage_tenant".to_string(),
            billing_period_start: Utc::now() - Duration::days(30),
            billing_period_end: Utc::now(),
            total_searches: 100000,
            total_insertions: 50000,
            total_ai_queries: 15000,                       // High AI usage
            total_storage_bytes: 200 * 1024 * 1024 * 1024, // 200GB - high storage
            total_compute_ms: 10000000,
            total_cost: 6000.0, // High cost - qualifies for enterprise tier
            usage_by_day: HashMap::new(),
            peak_usage_metrics: PeakUsageMetrics {
                peak_qps: 500.0,
                peak_concurrent_users: 50,
                peak_storage_gb: 200.0,
                peak_ai_requests_per_hour: 100,
            },
        };

        let recommendations = engine
            .generate_cost_optimization_recommendations(&high_usage_aggregate)
            .await
            .unwrap();

        assert!(!recommendations.is_empty());
        assert!(recommendations.iter().any(|r| matches!(
            r.recommendation_type,
            RecommendationType::StorageOptimization
        )));
        assert!(
            recommendations
                .iter()
                .any(|r| matches!(r.recommendation_type, RecommendationType::PlanUpgrade))
        );
    }
}
