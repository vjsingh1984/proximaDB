//! Business Intelligence Engine
//!
//! Main engine for AI-powered business intelligence, implementing
//! automated insight generation and executive dashboard creation.

use super::insight_generator::{BusinessInsight, InsightGenerator};
use super::report_generator::ReportGenerator;
use super::trend_analyzer::{TrendAnalysis, TrendAnalyzer};
use crate::ai::llm_integration::types::LLMRequestContext;
use crate::ai::llm_integration::{LLMIntegrationEngine, LLMRequest};
use crate::ai::natural_language::NLQueryTranslator;
use crate::ai::natural_language::translator::UserContext;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info, warn};

/// Main Business Intelligence Engine
#[derive(Clone)]
pub struct BusinessIntelligenceEngine {
    llm_engine: Arc<LLMIntegrationEngine>,
    nl_translator: Arc<NLQueryTranslator>,
    insight_generator: Arc<InsightGenerator>,
    _report_generator: Arc<ReportGenerator>,
    trend_analyzer: Arc<TrendAnalyzer>,
    config: BIEngineConfig,
}

/// Configuration for Business Intelligence Engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BIEngineConfig {
    /// Whether automated insight generation is enabled
    pub enable_automated_insights: bool,
    /// Whether executive dashboard generation is enabled
    pub enable_executive_dashboards: bool,
    /// Whether trend analysis is enabled
    pub enable_trend_analysis: bool,
    /// Interval for refreshing insights in minutes
    pub insight_refresh_interval_minutes: u32,
    /// Maximum number of insights per report
    pub max_insights_per_report: usize,
    /// Whether predictive analytics is enabled
    pub enable_predictive_analytics: bool,
}

/// Executive dashboard data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutiveDashboard {
    /// Executive summary overview
    pub summary: ExecutiveSummary,
    /// Key business metrics
    pub key_metrics: BusinessMetrics,
    /// Trend analysis results
    pub trends: Vec<TrendAnalysis>,
    /// Business insights generated
    pub insights: Vec<BusinessInsight>,
    /// Actionable recommendations
    pub recommendations: Vec<BusinessRecommendation>,
    /// Timestamp of dashboard generation
    pub generated_at: DateTime<Utc>,
    /// Tenant this dashboard belongs to
    pub tenant_id: String,
    /// Unique dashboard identifier
    pub dashboard_id: String,
}

/// Executive summary for dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutiveSummary {
    /// Overall performance status
    pub performance_status: PerformanceStatus,
    /// Key achievements in the period
    pub key_achievements: Vec<String>,
    /// Critical alerts requiring attention
    pub critical_alerts: Vec<String>,
    /// Growth indicator metrics
    pub growth_indicators: GrowthIndicators,
}

/// Business metrics collection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessMetrics {
    /// Revenue-related metrics
    pub revenue_metrics: RevenueMetrics,
    /// Customer-related metrics
    pub customer_metrics: CustomerMetrics,
    /// Operational efficiency metrics
    pub operational_metrics: OperationalMetrics,
    /// Performance and throughput metrics
    pub performance_metrics: PerformanceMetrics,
}

/// Revenue-related metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct RevenueMetrics {
    /// Total revenue amount
    pub total_revenue: Option<f64>,
    /// Revenue growth rate as a percentage
    pub revenue_growth_percent: Option<f64>,
    /// Average revenue per customer
    pub avg_revenue_per_customer: Option<f64>,
    /// Revenue breakdown by business segment
    pub revenue_by_segment: HashMap<String, f64>,
}

/// Customer-related metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct CustomerMetrics {
    /// Total number of customers
    pub total_customers: Option<u64>,
    /// Number of new customers acquired
    pub new_customers: Option<u64>,
    /// Customer churn rate as a percentage
    pub churn_rate_percent: Option<f64>,
    /// Customer satisfaction score (e.g., NPS)
    pub customer_satisfaction_score: Option<f64>,
}

/// Operational metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct OperationalMetrics {
    /// System uptime percentage
    pub system_uptime_percent: Option<f64>,
    /// Average response time in milliseconds
    pub average_response_time_ms: Option<f64>,
    /// Error rate as a percentage
    pub error_rate_percent: Option<f64>,
    /// Resource utilization percentage
    pub resource_utilization_percent: Option<f64>,
}

/// Performance metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct PerformanceMetrics {
    /// Queries processed per second
    pub queries_per_second: Option<f64>,
    /// Cache hit rate percentage
    pub cache_hit_rate_percent: Option<f64>,
    /// Storage efficiency percentage
    pub storage_efficiency_percent: Option<f64>,
    /// Number of concurrent users
    pub concurrent_users: Option<u32>,
}

/// Performance status indicator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PerformanceStatus {
    /// Exceeding all targets
    Excellent,
    /// Meeting targets
    Good,
    /// Within acceptable range
    Acceptable,
    /// Below targets, needs attention
    NeedsAttention,
    /// Critically below targets
    Critical,
}

/// Growth indicators
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthIndicators {
    /// User growth rate percentage
    pub user_growth_percent: Option<f64>,
    /// Revenue growth rate percentage
    pub revenue_growth_percent: Option<f64>,
    /// Data volume growth rate percentage
    pub data_growth_percent: Option<f64>,
    /// Market share percentage
    pub market_share_percent: Option<f64>,
}

/// Business recommendation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessRecommendation {
    /// Brief title of the recommendation
    pub title: String,
    /// Detailed description
    pub description: String,
    /// Priority level
    pub priority: RecommendationPriority,
    /// Estimated business impact description
    pub estimated_impact: String,
    /// Implementation effort estimate
    pub implementation_effort: ImplementationEffort,
    /// Suggested implementation timeline
    pub timeline: String,
}

/// Priority levels for recommendations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecommendationPriority {
    /// Low priority, can be deferred
    Low,
    /// Medium priority
    Medium,
    /// High priority, should be addressed soon
    High,
    /// Critical priority, requires immediate attention
    Critical,
}

/// Implementation effort estimate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImplementationEffort {
    /// Low effort, quick to implement
    Low,
    /// Medium effort
    Medium,
    /// High effort, significant resources needed
    High,
    /// Very high effort, major initiative
    VeryHigh,
}

/// Errors for Business Intelligence operations
#[derive(Debug, Error, Clone)]
pub enum BIError {
    #[error("Data extraction failed: {reason}")]
    DataExtractionFailed { reason: String },

    #[error("Insight generation failed: {reason}")]
    InsightGenerationFailed { reason: String },

    #[error("Report generation failed: {reason}")]
    ReportGenerationFailed { reason: String },

    #[error("Trend analysis failed: {reason}")]
    TrendAnalysisFailed { reason: String },

    #[error("Permission denied for user {user_id}: {reason}")]
    PermissionDenied { user_id: String, reason: String },

    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Internal BI error: {0}")]
    InternalError(String),
}

impl BusinessIntelligenceEngine {
    /// Create new Business Intelligence Engine
    pub async fn new(
        llm_engine: Arc<LLMIntegrationEngine>,
        nl_translator: Arc<NLQueryTranslator>,
        config: BIEngineConfig,
    ) -> Result<Self, BIError> {
        let insight_generator = Arc::new(InsightGenerator::new(llm_engine.clone()).await.map_err(
            |e| BIError::ConfigurationError(format!("Failed to create insight generator: {}", e)),
        )?);

        let report_generator = Arc::new(ReportGenerator::new(llm_engine.clone()).await.map_err(
            |e| BIError::ConfigurationError(format!("Failed to create report generator: {}", e)),
        )?);

        let trend_analyzer = Arc::new(TrendAnalyzer::new().await.map_err(|e| {
            BIError::ConfigurationError(format!("Failed to create trend analyzer: {}", e))
        })?);

        Ok(Self {
            llm_engine,
            nl_translator,
            insight_generator,
            _report_generator: report_generator,
            trend_analyzer,
            config,
        })
    }

    /// Generate comprehensive executive dashboard
    pub async fn generate_executive_dashboard(
        &self,
        user_context: &UserContext,
    ) -> Result<ExecutiveDashboard, BIError> {
        info!(
            "Generating executive dashboard for user: {}",
            user_context.user_id
        );

        // Step 1: Extract business metrics
        let business_metrics = self.extract_business_metrics(user_context).await?;

        // Step 2: Perform trend analysis
        let trends = if self.config.enable_trend_analysis {
            self.trend_analyzer
                .analyze_business_trends(&business_metrics)
                .await
                .map_err(|e| BIError::TrendAnalysisFailed {
                    reason: e.to_string(),
                })?
        } else {
            vec![]
        };

        // Step 3: Generate automated insights
        let insights = if self.config.enable_automated_insights {
            self.insight_generator
                .generate_business_insights(&business_metrics, &trends)
                .await
                .map_err(|e| BIError::InsightGenerationFailed {
                    reason: e.to_string(),
                })?
        } else {
            vec![]
        };

        // Step 4: Generate executive summary
        let summary = self
            .generate_executive_summary(&business_metrics, &insights)
            .await?;

        // Step 5: Generate recommendations
        let recommendations = self
            .generate_business_recommendations(&insights, &trends)
            .await?;

        let dashboard = ExecutiveDashboard {
            summary,
            key_metrics: business_metrics,
            trends,
            insights: insights
                .into_iter()
                .take(self.config.max_insights_per_report)
                .collect(),
            recommendations,
            generated_at: Utc::now(),
            tenant_id: user_context.tenant_id.clone().unwrap_or_default(),
            dashboard_id: uuid::Uuid::new_v4().to_string(),
        };

        info!(
            "Executive dashboard generated successfully: {} insights, {} trends, {} recommendations",
            dashboard.insights.len(),
            dashboard.trends.len(),
            dashboard.recommendations.len()
        );

        Ok(dashboard)
    }

    /// Extract key business metrics using natural language queries
    async fn extract_business_metrics(
        &self,
        user_context: &UserContext,
    ) -> Result<BusinessMetrics, BIError> {
        debug!(
            "Extracting business metrics for tenant: {:?}",
            user_context.tenant_id
        );

        // Define business intelligence queries
        let bi_queries = vec![
            "What is the total revenue for this month?",
            "How many new customers did we acquire this week?",
            "What is our customer retention rate?",
            "What are the top performing products by revenue?",
            "What is our system uptime percentage?",
            "What is the average query response time?",
            "How many queries per second are we processing?",
            "What is our cache hit rate?",
        ];

        let mut revenue_metrics = RevenueMetrics {
            total_revenue: None,
            revenue_growth_percent: None,
            avg_revenue_per_customer: None,
            revenue_by_segment: HashMap::new(),
        };

        let mut customer_metrics = CustomerMetrics {
            total_customers: None,
            new_customers: None,
            churn_rate_percent: None,
            customer_satisfaction_score: None,
        };

        let mut operational_metrics = OperationalMetrics {
            system_uptime_percent: None,
            average_response_time_ms: None,
            error_rate_percent: None,
            resource_utilization_percent: None,
        };

        let mut performance_metrics = PerformanceMetrics {
            queries_per_second: None,
            cache_hit_rate_percent: None,
            storage_efficiency_percent: None,
            concurrent_users: None,
        };

        // Execute business intelligence queries
        for query in bi_queries {
            match self
                .nl_translator
                .translate_to_sql(query, user_context)
                .await
            {
                Ok(translation) => {
                    debug!("Translated BI query: {} -> {}", query, translation.sql);

                    // Execute SQL and extract metrics (placeholder for actual execution)
                    if let Ok(result) = self.execute_business_query(&translation.sql).await {
                        self.process_business_query_result(
                            query,
                            &result,
                            &mut revenue_metrics,
                            &mut customer_metrics,
                            &mut operational_metrics,
                            &mut performance_metrics,
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to translate BI query '{}': {}", query, e);
                }
            }
        }

        Ok(BusinessMetrics {
            revenue_metrics,
            customer_metrics,
            operational_metrics,
            performance_metrics,
        })
    }

    /// Execute business query (placeholder for actual database execution)
    async fn execute_business_query(&self, _sql: &str) -> Result<QueryResult, BIError> {
        // Placeholder implementation
        // In real implementation, would execute SQL against ProximaDB
        Ok(QueryResult {
            rows: vec![],
            columns: vec![],
            execution_time_ms: 100,
        })
    }

    /// Process business query results and update metrics
    fn process_business_query_result(
        &self,
        query: &str,
        result: &QueryResult,
        revenue_metrics: &mut RevenueMetrics,
        customer_metrics: &mut CustomerMetrics,
        operational_metrics: &mut OperationalMetrics,
        performance_metrics: &mut PerformanceMetrics,
    ) {
        // Simple pattern matching to categorize and process results
        let query_lower = query.to_lowercase();

        if query_lower.contains("revenue") {
            // Process revenue-related results
            if let Some(value) = result.extract_numeric_value() {
                if query_lower.contains("total") {
                    revenue_metrics.total_revenue = Some(value);
                } else if query_lower.contains("growth") {
                    revenue_metrics.revenue_growth_percent = Some(value);
                }
            }
        } else if query_lower.contains("customer") {
            // Process customer-related results
            if let Some(value) = result.extract_numeric_value() {
                if query_lower.contains("new") {
                    customer_metrics.new_customers = Some(value as u64);
                } else if query_lower.contains("total") {
                    customer_metrics.total_customers = Some(value as u64);
                } else if query_lower.contains("retention") || query_lower.contains("churn") {
                    customer_metrics.churn_rate_percent = Some(value);
                }
            }
        } else if query_lower.contains("uptime") || query_lower.contains("response time") {
            // Process operational metrics
            if let Some(value) = result.extract_numeric_value() {
                if query_lower.contains("uptime") {
                    operational_metrics.system_uptime_percent = Some(value);
                } else if query_lower.contains("response time") {
                    operational_metrics.average_response_time_ms = Some(value);
                }
            }
        } else if query_lower.contains("queries per second") || query_lower.contains("cache") {
            // Process performance metrics
            if let Some(value) = result.extract_numeric_value() {
                if query_lower.contains("queries per second") {
                    performance_metrics.queries_per_second = Some(value);
                } else if query_lower.contains("cache hit") {
                    performance_metrics.cache_hit_rate_percent = Some(value);
                }
            }
        }
    }

    /// Generate executive summary
    async fn generate_executive_summary(
        &self,
        metrics: &BusinessMetrics,
        insights: &[BusinessInsight],
    ) -> Result<ExecutiveSummary, BIError> {
        // Determine performance status
        let performance_status = self.calculate_performance_status(metrics);

        // Extract key achievements from insights
        let key_achievements = insights
            .iter()
            .filter(|insight| insight.impact_score > 0.8)
            .map(|insight| insight.title.clone())
            .take(5)
            .collect();

        // Extract critical alerts
        let critical_alerts = insights
            .iter()
            .filter(|insight| {
                insight.confidence_score < 0.5
                    || insight.description.to_lowercase().contains("critical")
            })
            .map(|insight| insight.description.clone())
            .take(3)
            .collect();

        // Calculate growth indicators
        let growth_indicators = GrowthIndicators {
            user_growth_percent: self.calculate_user_growth(metrics),
            revenue_growth_percent: metrics.revenue_metrics.revenue_growth_percent,
            data_growth_percent: self.calculate_data_growth(metrics),
            market_share_percent: None, // Would require external data
        };

        Ok(ExecutiveSummary {
            performance_status,
            key_achievements,
            critical_alerts,
            growth_indicators,
        })
    }

    /// Calculate overall performance status
    fn calculate_performance_status(&self, metrics: &BusinessMetrics) -> PerformanceStatus {
        let mut score = 0;
        let mut factors = 0;

        // Revenue performance
        if let Some(revenue_growth) = metrics.revenue_metrics.revenue_growth_percent {
            factors += 1;
            if revenue_growth > 20.0 {
                score += 2;
            } else if revenue_growth > 10.0 {
                score += 1;
            } else if revenue_growth < -5.0 {
                score -= 1;
            }
        }

        // Customer metrics
        if let Some(churn_rate) = metrics.customer_metrics.churn_rate_percent {
            factors += 1;
            if churn_rate < 5.0 {
                score += 2;
            } else if churn_rate < 10.0 {
                score += 1;
            } else if churn_rate > 20.0 {
                score -= 1;
            }
        }

        // System performance
        if let Some(uptime) = metrics.operational_metrics.system_uptime_percent {
            factors += 1;
            if uptime > 99.9 {
                score += 2;
            } else if uptime > 99.0 {
                score += 1;
            } else if uptime < 95.0 {
                score -= 2;
            }
        }

        // Response time
        if let Some(response_time) = metrics.operational_metrics.average_response_time_ms {
            factors += 1;
            if response_time < 100.0 {
                score += 2;
            } else if response_time < 500.0 {
                score += 1;
            } else if response_time > 2000.0 {
                score -= 1;
            }
        }

        // Calculate average score
        let avg_score = if factors > 0 {
            score as f64 / factors as f64
        } else {
            0.0
        };

        match avg_score {
            s if s >= 1.5 => PerformanceStatus::Excellent,
            s if s >= 0.5 => PerformanceStatus::Good,
            s if s >= -0.5 => PerformanceStatus::Acceptable,
            s if s >= -1.5 => PerformanceStatus::NeedsAttention,
            _ => PerformanceStatus::Critical,
        }
    }

    /// Generate business recommendations based on insights and trends
    async fn generate_business_recommendations(
        &self,
        insights: &[BusinessInsight],
        trends: &[TrendAnalysis],
    ) -> Result<Vec<BusinessRecommendation>, BIError> {
        let mut recommendations = Vec::new();

        // Analyze insights for recommendation opportunities
        for insight in insights {
            if insight.impact_score > 0.7 {
                let recommendation = self.generate_recommendation_from_insight(insight).await?;
                recommendations.push(recommendation);
            }
        }

        // Analyze trends for strategic recommendations
        for trend in trends {
            if let Some(recommendation) = self.generate_recommendation_from_trend(trend).await? {
                recommendations.push(recommendation);
            }
        }

        // Sort by priority and limit
        recommendations.sort_by(|a, b| {
            let a_priority = match a.priority {
                RecommendationPriority::Critical => 4,
                RecommendationPriority::High => 3,
                RecommendationPriority::Medium => 2,
                RecommendationPriority::Low => 1,
            };
            let b_priority = match b.priority {
                RecommendationPriority::Critical => 4,
                RecommendationPriority::High => 3,
                RecommendationPriority::Medium => 2,
                RecommendationPriority::Low => 1,
            };
            b_priority.cmp(&a_priority)
        });

        recommendations.truncate(10); // Limit to top 10 recommendations

        Ok(recommendations)
    }

    /// Generate recommendation from business insight
    async fn generate_recommendation_from_insight(
        &self,
        insight: &BusinessInsight,
    ) -> Result<BusinessRecommendation, BIError> {
        // Use LLM to generate actionable recommendation
        let prompt = format!(
            "Based on this business insight, generate a specific, actionable recommendation:

Insight: {}
Description: {}
Impact Score: {:.2}

Generate a business recommendation that includes:
1. Specific action to take
2. Expected business impact
3. Implementation timeline
4. Required resources

Recommendation:",
            insight.title, insight.description, insight.impact_score
        );

        let llm_request = LLMRequest::new(prompt)
            .with_max_tokens(300)
            .with_temperature(0.7);

        let context = LLMRequestContext::new(uuid::Uuid::new_v4().to_string());

        match self
            .llm_engine
            .query_with_fallback_and_context(&llm_request, &context)
            .await
        {
            Ok(response) => Ok(BusinessRecommendation {
                title: insight.title.clone(),
                description: response.content,
                priority: self.map_impact_to_priority(insight.impact_score),
                estimated_impact: format!("Impact Score: {:.2}", insight.impact_score),
                implementation_effort: ImplementationEffort::Medium,
                timeline: "2-4 weeks".to_string(),
            }),
            Err(e) => {
                warn!("Failed to generate recommendation from LLM: {}", e);
                // Fallback to template-based recommendation
                Ok(BusinessRecommendation {
                    title: insight.title.clone(),
                    description: format!("Review and address: {}", insight.description),
                    priority: self.map_impact_to_priority(insight.impact_score),
                    estimated_impact: format!("Impact Score: {:.2}", insight.impact_score),
                    implementation_effort: ImplementationEffort::Medium,
                    timeline: "1-2 weeks".to_string(),
                })
            }
        }
    }

    /// Generate recommendation from trend analysis
    async fn generate_recommendation_from_trend(
        &self,
        trend: &TrendAnalysis,
    ) -> Result<Option<BusinessRecommendation>, BIError> {
        // Only generate recommendations for significant trends
        if trend.confidence_score < 0.7 {
            return Ok(None);
        }

        let recommendation = BusinessRecommendation {
            title: format!("Address {} trend in {}", trend.direction, trend.metric_name),
            description: format!(
                "Trend analysis shows {} trend with {:.1}% change",
                trend.direction, trend.change_percentage
            ),
            priority: if trend.change_percentage.abs() > 20.0 {
                RecommendationPriority::High
            } else {
                RecommendationPriority::Medium
            },
            estimated_impact: format!("Trend strength: {:.2}", trend.confidence_score),
            implementation_effort: ImplementationEffort::Medium,
            timeline: "1-3 weeks".to_string(),
        };

        Ok(Some(recommendation))
    }

    /// Map impact score to recommendation priority
    fn map_impact_to_priority(&self, impact_score: f32) -> RecommendationPriority {
        match impact_score {
            s if s >= 0.9 => RecommendationPriority::Critical,
            s if s >= 0.7 => RecommendationPriority::High,
            s if s >= 0.5 => RecommendationPriority::Medium,
            _ => RecommendationPriority::Low,
        }
    }

    /// Calculate user growth percentage
    fn calculate_user_growth(&self, _metrics: &BusinessMetrics) -> Option<f64> {
        // Placeholder - would calculate from customer metrics
        None
    }

    /// Calculate data growth percentage
    fn calculate_data_growth(&self, _metrics: &BusinessMetrics) -> Option<f64> {
        // Placeholder - would calculate from storage metrics
        None
    }
}

/// Query result structure (placeholder)
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Result rows as key-value maps
    pub rows: Vec<HashMap<String, String>>,
    /// Column names in the result
    pub columns: Vec<String>,
    /// Query execution time in milliseconds
    pub execution_time_ms: u64,
}

impl QueryResult {
    /// Extract the first numeric value from the first row.
    pub fn extract_numeric_value(&self) -> Option<f64> {
        // Extract first numeric value from first row
        self.rows.first()?.values().next()?.parse().ok()
    }
}

impl Default for BIEngineConfig {
    fn default() -> Self {
        Self {
            enable_automated_insights: true,
            enable_executive_dashboards: true,
            enable_trend_analysis: true,
            insight_refresh_interval_minutes: 60,
            max_insights_per_report: 10,
            enable_predictive_analytics: true,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "Requires async runtime and complex mocking setup"]
    fn test_performance_status_calculation() {
        // This test requires an async runtime and complex mocking
        // of LLM engine, translator, and other async components
        // TODO: Implement with proper async test framework and mocks
    }
}




