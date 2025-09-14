//! License Tier Enforcement
//!
//! Runtime enforcement of license tier limits and feature restrictions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use anyhow::{Result, anyhow};
use tracing::{debug, info, warn};

/// License tier enforcement system
#[derive(Debug, Clone)]
pub struct TierEnforcement {
    tier_definitions: HashMap<String, TierDefinition>,
    enforcement_config: EnforcementConfig,
}

/// Configuration for license enforcement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnforcementConfig {
    pub strict_enforcement: bool,
    pub grace_period_hours: u32,
    pub warning_threshold_percentage: f64, // Warn at 80% of limit
    pub enable_usage_warnings: bool,
}

/// Tier definition with limits and features
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierDefinition {
    pub tier_name: String,
    pub tier_level: u32, // 0=Free, 1=Developer, 2=Professional, 3=Enterprise
    pub feature_limits: HashMap<String, FeatureLimit>,
    pub usage_quotas: HashMap<String, UsageQuota>,
    pub allowed_features: Vec<String>,
    pub restricted_features: Vec<String>,
}

/// Feature limit definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureLimit {
    pub limit_type: LimitType,
    pub limit_value: LimitValue,
    pub enforcement_action: EnforcementAction,
    pub warning_threshold: Option<f64>,
}

/// Types of limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LimitType {
    HardLimit,     // Absolute limit, cannot exceed
    SoftLimit,     // Warning limit, can exceed temporarily
    Rate,          // Rate limiting (requests per time period)
    Concurrent,    // Concurrent resource limits
}

/// Limit values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LimitValue {
    Count(u64),
    Percentage(f64),
    Rate { requests: u32, period_seconds: u32 },
    Boolean(bool),
}

/// Enforcement actions when limits are exceeded
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EnforcementAction {
    Block,         // Block the request
    Warn,          // Allow but warn
    Throttle,      // Rate limit the requests
    Redirect,      // Redirect to upgrade page
}

/// Usage quota tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageQuota {
    pub quota_name: String,
    pub limit: u64,
    pub current_usage: u64,
    pub reset_period: ResetPeriod,
    pub last_reset: DateTime<Utc>,
}

/// Reset periods for usage quotas
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResetPeriod {
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Never, // One-time limits
}

impl TierEnforcement {
    /// Create new tier enforcement system
    pub fn new() -> Self {
        let mut tier_definitions = HashMap::new();

        // Free tier definition
        tier_definitions.insert("Free".to_string(), TierDefinition {
            tier_name: "Free".to_string(),
            tier_level: 0,
            feature_limits: [
                ("ai_queries".to_string(), FeatureLimit {
                    limit_type: LimitType::Rate,
                    limit_value: LimitValue::Rate { requests: 50, period_seconds: 86400 }, // 50/day
                    enforcement_action: EnforcementAction::Block,
                    warning_threshold: Some(0.8),
                }),
                ("collections".to_string(), FeatureLimit {
                    limit_type: LimitType::HardLimit,
                    limit_value: LimitValue::Count(10),
                    enforcement_action: EnforcementAction::Block,
                    warning_threshold: Some(0.8),
                }),
                ("vectors".to_string(), FeatureLimit {
                    limit_type: LimitType::HardLimit,
                    limit_value: LimitValue::Count(100_000),
                    enforcement_action: EnforcementAction::Block,
                    warning_threshold: Some(0.9),
                }),
            ].iter().cloned().collect(),
            usage_quotas: HashMap::new(),
            allowed_features: vec![
                "basic_vector_search".to_string(),
                "basic_graph_queries".to_string(),
                "natural_language_queries".to_string(),
            ],
            restricted_features: vec![
                "executive_dashboards".to_string(),
                "enterprise_sso".to_string(),
                "unlimited_tenants".to_string(),
                "advanced_ai_features".to_string(),
            ],
        });

        // Professional tier definition
        tier_definitions.insert("Professional".to_string(), TierDefinition {
            tier_name: "Professional".to_string(),
            tier_level: 2,
            feature_limits: [
                ("ai_queries".to_string(), FeatureLimit {
                    limit_type: LimitType::Rate,
                    limit_value: LimitValue::Rate { requests: 10000, period_seconds: 86400 }, // 10K/day
                    enforcement_action: EnforcementAction::Throttle,
                    warning_threshold: Some(0.9),
                }),
                ("collections".to_string(), FeatureLimit {
                    limit_type: LimitType::SoftLimit,
                    limit_value: LimitValue::Count(1000),
                    enforcement_action: EnforcementAction::Warn,
                    warning_threshold: Some(0.8),
                }),
            ].iter().cloned().collect(),
            usage_quotas: HashMap::new(),
            allowed_features: vec![
                "all_basic_features".to_string(),
                "executive_dashboards".to_string(),
                "advanced_ai_features".to_string(),
                "multi_tenant_enhanced".to_string(),
                "performance_monitoring".to_string(),
            ],
            restricted_features: vec![
                "unlimited_enterprise_features".to_string(),
                "custom_development".to_string(),
            ],
        });

        // Enterprise tier definition (unlimited)
        tier_definitions.insert("Enterprise".to_string(), TierDefinition {
            tier_name: "Enterprise".to_string(),
            tier_level: 3,
            feature_limits: HashMap::new(), // No limits
            usage_quotas: HashMap::new(),
            allowed_features: vec!["all_features".to_string()],
            restricted_features: vec![], // No restrictions
        });

        Self {
            tier_definitions,
            enforcement_config: EnforcementConfig::default(),
        }
    }

    /// Enforce license tier limits
    pub async fn enforce_tier_limits(&self, tier: &str, feature: &str, current_usage: u64) -> Result<EnforcementResult> {
        debug!("🔍 Enforcing tier limits: {} for feature {} (usage: {})", tier, feature, current_usage);

        let tier_def = self.tier_definitions.get(tier)
            .ok_or_else(|| anyhow!("Unknown tier: {}", tier))?;

        // Check if feature is allowed at all
        if tier_def.restricted_features.contains(&feature.to_string()) {
            return Ok(EnforcementResult {
                allowed: false,
                action: EnforcementAction::Block,
                reason: format!("Feature '{}' not available in {} tier", feature, tier),
                usage_percentage: 0.0,
                upgrade_recommendation: Some(self.recommend_upgrade_for_feature(feature)),
            });
        }

        // Check feature limits
        if let Some(limit) = tier_def.feature_limits.get(feature) {
            let (allowed, usage_percentage) = self.check_limit_compliance(current_usage, limit)?;

            if !allowed {
                return Ok(EnforcementResult {
                    allowed: false,
                    action: limit.enforcement_action.clone(),
                    reason: format!("Usage limit exceeded for '{}': {} > {:?}", feature, current_usage, limit.limit_value),
                    usage_percentage,
                    upgrade_recommendation: Some(self.recommend_upgrade_for_feature(feature)),
                });
            }

            // Check warning threshold
            if let Some(warning_threshold) = limit.warning_threshold {
                if usage_percentage > warning_threshold && self.enforcement_config.enable_usage_warnings {
                    warn!("⚠️ Usage warning: {} at {:.1}% of limit", feature, usage_percentage * 100.0);
                }
            }
        }

        Ok(EnforcementResult {
            allowed: true,
            action: EnforcementAction::Allow,
            reason: "Within license limits".to_string(),
            usage_percentage: 0.0,
            upgrade_recommendation: None,
        })
    }

    /// Check compliance with specific limit
    fn check_limit_compliance(&self, current_usage: u64, limit: &FeatureLimit) -> Result<(bool, f64)> {
        match &limit.limit_value {
            LimitValue::Count(max_count) => {
                let usage_percentage = current_usage as f64 / *max_count as f64;
                Ok((current_usage <= *max_count, usage_percentage))
            }
            LimitValue::Rate { requests, period_seconds: _ } => {
                // Simplified rate check - would need proper rate limiting in production
                Ok((current_usage <= *requests as u64, current_usage as f64 / *requests as f64))
            }
            LimitValue::Boolean(allowed) => {
                Ok((*allowed, if *allowed { 0.0 } else { 1.0 }))
            }
            LimitValue::Percentage(max_percentage) => {
                Ok((current_usage as f64 <= *max_percentage, current_usage as f64 / max_percentage))
            }
        }
    }

    /// Recommend license upgrade for specific feature
    fn recommend_upgrade_for_feature(&self, feature: &str) -> String {
        match feature {
            f if f.contains("ai") || f.contains("executive") => "Professional".to_string(),
            f if f.contains("enterprise") || f.contains("sso") => "Enterprise".to_string(),
            f if f.contains("unlimited") || f.contains("custom") => "Enterprise".to_string(),
            _ => "Professional".to_string(),
        }
    }
}

/// Enforcement result
#[derive(Debug, Clone)]
pub struct EnforcementResult {
    pub allowed: bool,
    pub action: EnforcementAction,
    pub reason: String,
    pub usage_percentage: f64,
    pub upgrade_recommendation: Option<String>,
}

/// Allow action for positive enforcement results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Allow {
    Allow,
}

impl Default for EnforcementConfig {
    fn default() -> Self {
        Self {
            strict_enforcement: true,
            grace_period_hours: 24,
            warning_threshold_percentage: 0.8,
            enable_usage_warnings: true,
        }
    }
}

use chrono::{DateTime, Utc};
use super::license_manager::UsageContext;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_enforcement_creation() {
        let enforcement = TierEnforcement::new();
        assert!(enforcement.tier_definitions.contains_key("Free"));
        assert!(enforcement.tier_definitions.contains_key("Professional"));
        assert!(enforcement.tier_definitions.contains_key("Enterprise"));
    }

    #[tokio::test]
    async fn test_free_tier_limits() {
        let enforcement = TierEnforcement::new();

        // Test AI query limits for free tier
        let result = enforcement.enforce_tier_limits("Free", "ai_queries", 60).await.unwrap(); // Over 50/day limit
        assert!(!result.allowed);
        assert!(matches!(result.action, EnforcementAction::Block));

        // Test within limits
        let result = enforcement.enforce_tier_limits("Free", "ai_queries", 30).await.unwrap(); // Under 50/day limit
        assert!(result.allowed);
    }

    #[tokio::test]
    async fn test_enterprise_tier_unlimited() {
        let enforcement = TierEnforcement::new();

        // Enterprise tier should have no limits
        let result = enforcement.enforce_tier_limits("Enterprise", "ai_queries", 1000000).await.unwrap();
        assert!(result.allowed); // Unlimited usage
    }
}