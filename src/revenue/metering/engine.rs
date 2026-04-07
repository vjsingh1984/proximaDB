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
    /// Whether real-time usage metering is active.
    pub enable_real_time_metering: bool,
    /// Dollar threshold that triggers an immediate billing cycle.
    pub billing_threshold_usd: f64,
    /// Number of days between scheduled billing runs.
    pub billing_frequency_days: u32,
    /// Whether usage analytics dashboards are enabled.
    pub enable_usage_analytics: bool,
    /// Whether automated cost optimization recommendations are generated.
    pub enable_cost_optimization: bool,
}

/// Usage event for enterprise metering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    /// Unique identifier for this usage event.
    pub event_id: String,
    /// Tenant that generated the event.
    pub tenant_id: String,
    /// User within the tenant who triggered the event.
    pub user_id: String,
    /// Category of billable action performed.
    pub event_type: UsageEventType,
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Resource consumption metrics recorded for this event.
    pub resource_consumed: ResourceConsumption,
    /// Calculated dollar cost, populated after pricing evaluation.
    pub cost_impact: Option<f64>,
    /// Arbitrary key-value pairs for event context.
    pub metadata: HashMap<String, String>,
}

/// Types of billable usage events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UsageEventType {
    /// K-nearest-neighbor vector search with `k` results and given dimensionality.
    VectorSearch {
        /// Number of nearest neighbors requested.
        k: usize,
        /// Dimensionality of the query vector.
        dimensions: usize,
    },
    /// Batch vector insertion with count and dimensionality.
    VectorInsertion {
        /// Number of vectors inserted.
        count: usize,
        /// Dimensionality of each inserted vector.
        dimensions: usize,
    },
    /// New collection provisioned with an estimated storage footprint.
    CollectionCreation {
        /// Estimated storage size in bytes.
        estimated_size: u64,
    },
    /// AI-powered query routed to an LLM provider.
    AIQuery {
        /// LLM provider name (e.g. "openai").
        provider: String,
        /// Token count consumed by the query.
        tokens: u32,
    },
    /// Persistent data storage consumption snapshot.
    DataStorage {
        /// Current storage usage in bytes.
        bytes: u64,
    },
    /// Compute time consumed by a request.
    ComputeTime {
        /// Wall-clock compute time in milliseconds.
        milliseconds: u64,
    },
    /// Network data transfer egress.
    DataTransfer {
        /// Bytes transferred out.
        bytes: u64,
    },
    /// Executive analytics dashboard generation.
    ExecutiveDashboard {
        /// Number of insights produced in this dashboard run.
        insights_generated: u32,
    },
    /// Natural language query translated and executed.
    NaturalLanguageQuery {
        /// Estimated complexity score (0-100).
        complexity_score: u32,
    },
}

/// Resource consumption metrics for precise billing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceConsumption {
    /// CPU time consumed in milliseconds.
    pub cpu_milliseconds: u64,
    /// Peak memory used in bytes.
    pub memory_bytes: u64,
    /// Disk I/O in bytes.
    pub storage_bytes: u64,
    /// Network I/O in bytes.
    pub network_bytes: u64,
    /// LLM tokens consumed, if any.
    pub ai_tokens: u32,
    /// Application-defined numeric metrics.
    pub custom_metrics: HashMap<String, f64>,
}

/// Aggregated usage for billing periods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageAggregate {
    /// Tenant this aggregate belongs to.
    pub tenant_id: String,
    /// Start of the current billing period.
    pub billing_period_start: DateTime<Utc>,
    /// End of the current billing period.
    pub billing_period_end: DateTime<Utc>,
    /// Total vector search operations in this period.
    pub total_searches: u64,
    /// Total vectors inserted in this period.
    pub total_insertions: u64,
    /// Total AI-powered queries in this period.
    pub total_ai_queries: u64,
    /// High-water mark for storage consumption in bytes.
    pub total_storage_bytes: u64,
    /// Cumulative compute time in milliseconds.
    pub total_compute_ms: u64,
    /// Running dollar cost for the billing period.
    pub total_cost: f64,
    /// Per-day usage breakdown keyed by "YYYY-MM-DD".
    pub usage_by_day: HashMap<String, DailyUsage>,
    /// Observed peak resource usage for capacity planning.
    pub peak_usage_metrics: PeakUsageMetrics,
}

/// Daily usage breakdown for analytics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyUsage {
    /// Calendar date in "YYYY-MM-DD" format.
    pub date: String,
    /// Number of vector searches performed on this day.
    pub searches: u64,
    /// Number of vector insertions performed on this day.
    pub insertions: u64,
    /// Number of AI queries executed on this day.
    pub ai_queries: u64,
    /// Number of executive dashboard reports generated.
    pub executive_dashboards: u32,
    /// Total dollar cost accrued on this day.
    pub cost: f64,
    /// Peak queries-per-second observed during the day.
    pub peak_qps: f64,
}

/// Peak usage metrics for capacity planning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeakUsageMetrics {
    /// Peak queries per second during the period.
    pub peak_qps: f64,
    /// Peak number of concurrent users.
    pub peak_concurrent_users: u32,
    /// Peak storage usage in gigabytes.
    pub peak_storage_gb: f64,
    /// Peak AI requests per hour.
    pub peak_ai_requests_per_hour: u32,
}

/// Pricing configuration for enterprise billing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    /// Cost per 1000 vector searches.
    pub search_cost_per_1k: f64,
    /// Cost per 1000 vector insertions.
    pub insertion_cost_per_1k: f64,
    /// Cost per 1000 AI tokens.
    pub ai_cost_per_1k_tokens: f64,
    /// Monthly cost per GB of storage.
    pub storage_cost_per_gb: f64,
    /// Cost per compute hour.
    pub compute_cost_per_hour: f64,
    /// Cost per GB of data transfer.
    pub transfer_cost_per_gb: f64,
    /// Base cost for collection creation.
    pub collection_creation_base_cost: f64,
    /// Cost per executive dashboard generation.
    pub executive_dashboard_cost: f64,
    /// Cost per natural language query.
    pub natural_language_query_cost: f64,
    /// Cost threshold that triggers billing.
    pub billing_threshold: f64,
    /// Billing cycle length in days.
    pub billing_frequency_days: u32,
    /// Volume discount tiers for enterprise customers.
    pub enterprise_discount_tiers: Vec<DiscountTier>,
}

/// Discount tiers for enterprise customers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscountTier {
    /// Name of the discount tier (e.g., "Silver", "Gold", "Platinum").
    pub name: String,
    /// Minimum monthly spend in USD to qualify.
    pub min_monthly_spend: f64,
    /// Discount percentage applied to the total bill.
    pub discount_percentage: f64,
    /// Additional benefits included in this tier.
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
    /// Create a new daily usage record for the given date string (YYYY-MM-DD).
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
    /// Process billing for a tenant based on their aggregated usage.
    async fn process_billing(&self, tenant_id: &str, usage: &UsageAggregate) -> Result<()>;
}

/// Billing period options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BillingPeriod {
    /// Daily billing cycle.
    Daily,
    /// Weekly billing cycle.
    Weekly,
    /// Monthly billing cycle.
    Monthly,
    /// Quarterly billing cycle.
    Quarterly,
    /// Annual billing cycle.
    Annual,
    /// Custom billing period with explicit start and end dates.
    Custom {
        /// Start of the billing period.
        start: DateTime<Utc>,
        /// End of the billing period.
        end: DateTime<Utc>,
    },
}

/// Usage summary for customer dashboards
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Billing period this summary covers.
    pub period: BillingPeriod,
    /// Total cost in USD for this period.
    pub total_cost: f64,
    /// Breakdown of costs by category.
    pub usage_breakdown: UsageBreakdown,
    /// Usage trends and forecasting data.
    pub trending: UsageTrends,
    /// Cost optimization recommendations.
    pub recommendations: Vec<CostOptimizationRecommendation>,
    /// Peak usage metrics during this period.
    pub peak_metrics: PeakUsageMetrics,
}

/// Cost breakdown by category
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageBreakdown {
    /// Percentage of total cost from storage.
    pub storage_cost_percentage: f64,
    /// Percentage of total cost from searches.
    pub search_cost_percentage: f64,
    /// Percentage of total cost from AI operations.
    pub ai_cost_percentage: f64,
    /// Top cost drivers identified in the period.
    pub top_cost_drivers: Vec<String>,
}

/// Usage trends for forecasting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageTrends {
    /// Week-over-week growth rate as a percentage.
    pub week_over_week_growth: f64,
    /// Overall cost trend direction.
    pub cost_trend: TrendDirection,
    /// Detected usage pattern.
    pub usage_pattern: UsagePattern,
    /// Forecasted cost for the next month in USD.
    pub forecast_next_month: f64,
}

/// Trend directions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrendDirection {
    /// Costs are increasing over time.
    Increasing,
    /// Costs are decreasing over time.
    Decreasing,
    /// Costs are relatively stable.
    Stable,
}

/// Usage patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UsagePattern {
    /// Usage concentrated during business hours (9-5 weekdays).
    BusinessHours,
    /// Consistent usage around the clock.
    AlwaysOn,
    /// Unpredictable spikes in usage.
    Bursty,
    /// Usage concentrated on weekends.
    Weekend,
}

/// Cost optimization recommendations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostOptimizationRecommendation {
    /// Category of the recommendation.
    pub recommendation_type: RecommendationType,
    /// Short title for the recommendation.
    pub title: String,
    /// Detailed description of the optimization.
    pub description: String,
    /// Estimated monthly savings in USD.
    pub estimated_savings_usd: f64,
    /// Level of effort required to implement.
    pub implementation_effort: ImplementationEffort,
    /// Priority ranking for this recommendation.
    pub priority: RecommendationPriority,
}

/// Types of cost optimization recommendations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecommendationType {
    /// Optimize storage usage (compression, tiering, cleanup).
    StorageOptimization,
    /// Optimize AI token usage (caching, batching).
    AIOptimization,
    /// Upgrade to a more cost-effective plan.
    PlanUpgrade,
    /// Improve caching to reduce redundant operations.
    CacheOptimization,
    /// Optimize query patterns to reduce cost.
    QueryOptimization,
}

/// Implementation effort levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImplementationEffort {
    /// Minimal effort (configuration change or toggle).
    Low,
    /// Moderate effort (code or architecture changes).
    Medium,
    /// Significant effort (major redesign or migration).
    High,
}

/// Recommendation priority levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecommendationPriority {
    /// Low priority — nice to have.
    Low,
    /// Medium priority — should address soon.
    Medium,
    /// High priority — address this billing cycle.
    High,
    /// Critical — immediate action needed.
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
