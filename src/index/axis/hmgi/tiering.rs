/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! HMGI Tiering Integration
//!
//! Per-modality storage tiering for economies of scale.
//!
//! ## Tier Assignment Rules
//!
//! | Modality Access Pattern | Target Tier |
//! |------------------------|-------------|
//! | Hot (>100 QPS) | Memory/NVMe |
//! | Warm (10-100 QPS) | NVMe |
//! | Cold (<10 QPS) | CloudStandard |
//! | Archive | CloudArchive/DeepArchive |

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::infrastructure::tier_policy_engine::{
    AccessPatternMetrics, InfrastructureTier, SmartTierPolicy,
};

/// HMGI tier policy - modality-aware storage placement
///
/// Applies modality-specific overrides and access-pattern based placement.
pub struct HmgiTierPolicy {
    /// Modality-specific tier overrides
    modality_tiers: Arc<RwLock<HashMap<String, InfrastructureTier>>>,

    /// Access pattern tracking per modality
    access_patterns: Arc<RwLock<HashMap<String, AccessPatternMetrics>>>,

    /// Default tier for new modalities
    default_tier: InfrastructureTier,
}

impl HmgiTierPolicy {
    /// Create a new HMGI tier policy
    pub fn new(_base_policy: SmartTierPolicy) -> Self {
        Self {
            modality_tiers: Arc::new(RwLock::new(HashMap::new())),
            access_patterns: Arc::new(RwLock::new(HashMap::new())),
            default_tier: InfrastructureTier::NvmeSsd {
                mount_path: "/data/nvme".to_string(),
            },
        }
    }

    /// Create with custom default tier
    pub fn with_default_tier(
        _base_policy: SmartTierPolicy,
        default_tier: InfrastructureTier,
    ) -> Self {
        Self {
            modality_tiers: Arc::new(RwLock::new(HashMap::new())),
            access_patterns: Arc::new(RwLock::new(HashMap::new())),
            default_tier,
        }
    }

    /// Set tier override for a specific modality
    pub async fn set_modality_tier(&self, modality: String, tier: InfrastructureTier) {
        let mut modality_tiers = self.modality_tiers.write().await;
        modality_tiers.insert(modality, tier);
    }

    /// Get tier override for a modality
    pub async fn get_modality_tier(&self, modality: &str) -> Option<InfrastructureTier> {
        let modality_tiers = self.modality_tiers.read().await;
        modality_tiers.get(modality).cloned()
    }

    /// Clear tier override for a modality
    pub async fn clear_modality_tier(&self, modality: &str) {
        let mut modality_tiers = self.modality_tiers.write().await;
        modality_tiers.remove(modality);
    }

    /// Determine appropriate tier for a modality based on access patterns
    ///
    /// ## Tier Selection Logic
    ///
    /// 1. Check for manual override
    /// 2. If no override, use access patterns to auto-select
    /// 3. Fall back to default tier
    pub async fn select_tier_for_modality(&self, modality: &str) -> InfrastructureTier {
        // Check for manual override first
        {
            let modality_tiers = self.modality_tiers.read().await;
            if let Some(tier) = modality_tiers.get(modality) {
                return tier.clone();
            }
        }

        // Get access patterns for this modality
        let patterns = self.access_patterns.read().await;
        if let Some(pattern) = patterns.get(modality) {
            return self.select_tier_from_pattern(pattern);
        }

        // Fall back to default
        self.default_tier.clone()
    }

    /// Select tier based on access pattern metrics
    fn select_tier_from_pattern(&self, pattern: &AccessPatternMetrics) -> InfrastructureTier {
        // Hot tier: >100 QPS
        if pattern.hot_access_rate > 100.0 {
            return InfrastructureTier::Memory;
        }

        // Warm tier: 10-100 QPS
        if pattern.hot_access_rate >= 10.0 {
            return InfrastructureTier::NvmeSsd {
                mount_path: "/data/nvme".to_string(),
            };
        }

        // Cold tier: <10 QPS
        if pattern.hot_access_rate < 10.0 {
            return InfrastructureTier::CloudStandard {
                provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                    bucket: "proximadb-hmgi".to_string(),
                    storage_class:
                        crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            };
        }

        // Default to NVMe for unknown access shape
        InfrastructureTier::NvmeSsd {
            mount_path: "/data/nvme".to_string(),
        }
    }

    /// Record access for a modality
    ///
    /// Updates access patterns for tier auto-selection.
    pub async fn record_access(&self, modality: &str, is_read: bool) {
        let mut patterns = self.access_patterns.write().await;
        let pattern =
            patterns
                .entry(modality.to_string())
                .or_insert_with(|| AccessPatternMetrics {
                    hot_access_rate: 0.0,
                    warm_access_rate: 0.0,
                    cold_access_rate: 0.0,
                    sequential_access_pct: 0.0,
                    random_access_pct: 100.0,
                });

        // Simple increment-based tracking (in production, use sliding window)
        if is_read {
            pattern.hot_access_rate += 1.0;
        } else {
            pattern.warm_access_rate += 1.0;
        }
    }

    /// Get access patterns for all modalities
    pub async fn get_access_patterns(&self) -> HashMap<String, AccessPatternMetrics> {
        let patterns = self.access_patterns.read().await;
        patterns.clone()
    }

    /// Get access pattern for a specific modality
    pub async fn get_access_pattern(&self, modality: &str) -> Option<AccessPatternMetrics> {
        let patterns = self.access_patterns.read().await;
        patterns.get(modality).cloned()
    }

    /// Reset access patterns (for testing or manual correction)
    pub async fn reset_access_patterns(&self) {
        let mut patterns = self.access_patterns.write().await;
        patterns.clear();
    }

    /// Promote a modality to a hotter tier
    pub async fn promote_modality(
        &self,
        modality: &str,
        target_tier: InfrastructureTier,
    ) -> Result<TierChangeResult> {
        let current_tier = self.select_tier_for_modality(modality).await;

        // Validate promotion (only allow moving to hotter tiers)
        if target_tier.tier_level() >= current_tier.tier_level() {
            return Err(anyhow::anyhow!(
                "Cannot promote to colder tier: {:?} -> {:?}",
                current_tier,
                target_tier
            ));
        }

        // Set the new tier
        self.set_modality_tier(modality.to_string(), target_tier.clone())
            .await;

        Ok(TierChangeResult {
            modality: modality.to_string(),
            from_tier: current_tier,
            to_tier: target_tier,
            reason: TierChangeReason::Promotion,
        })
    }

    /// Demote a modality to a colder tier
    pub async fn demote_modality(
        &self,
        modality: &str,
        target_tier: InfrastructureTier,
    ) -> Result<TierChangeResult> {
        let current_tier = self.select_tier_for_modality(modality).await;

        // Validate demotion (only allow moving to colder tiers)
        if target_tier.tier_level() <= current_tier.tier_level() {
            return Err(anyhow::anyhow!(
                "Cannot demote to hotter tier: {:?} -> {:?}",
                current_tier,
                target_tier
            ));
        }

        // Set the new tier
        self.set_modality_tier(modality.to_string(), target_tier.clone())
            .await;

        Ok(TierChangeResult {
            modality: modality.to_string(),
            from_tier: current_tier,
            to_tier: target_tier,
            reason: TierChangeReason::Demotion,
        })
    }

    /// Auto-evaluate and adjust tiers based on access patterns
    ///
    /// Returns recommended tier changes for modalities that should move.
    pub async fn auto_evaluate_tiers(&self) -> Vec<TierChangeRecommendation> {
        let patterns = self.access_patterns.read().await;
        let mut recommendations = Vec::new();

        for (modality, pattern) in patterns.iter() {
            let current_tier = self.configured_tier_for_modality(modality).await;
            let recommended_tier = self.select_tier_from_pattern(pattern);

            if recommended_tier.tier_level() < current_tier.tier_level() {
                // Should promote
                recommendations.push(TierChangeRecommendation {
                    modality: modality.clone(),
                    current_tier: current_tier.clone(),
                    recommended_tier: recommended_tier.clone(),
                    reason: TierChangeReason::Promotion,
                    confidence: self.calculate_confidence(pattern, &recommended_tier),
                });
            } else if recommended_tier.tier_level() > current_tier.tier_level() {
                // Should demote
                recommendations.push(TierChangeRecommendation {
                    modality: modality.clone(),
                    current_tier: current_tier.clone(),
                    recommended_tier: recommended_tier.clone(),
                    reason: TierChangeReason::Demotion,
                    confidence: self.calculate_confidence(pattern, &recommended_tier),
                });
            }
        }

        recommendations
    }

    async fn configured_tier_for_modality(&self, modality: &str) -> InfrastructureTier {
        let modality_tiers = self.modality_tiers.read().await;
        modality_tiers
            .get(modality)
            .cloned()
            .unwrap_or_else(|| self.default_tier.clone())
    }

    /// Calculate confidence score for a tier change recommendation
    fn calculate_confidence(
        &self,
        pattern: &AccessPatternMetrics,
        tier: &InfrastructureTier,
    ) -> f32 {
        // Simple heuristic: higher confidence for more extreme access patterns
        match tier {
            InfrastructureTier::Memory => {
                let rate = pattern.hot_access_rate as f32;
                (rate / 200.0).min(1.0)
            }
            InfrastructureTier::CloudStandard { .. } => {
                let rate = pattern.hot_access_rate as f32;
                ((100.0 - rate) / 100.0).max(0.0).min(1.0)
            }
            _ => 0.5,
        }
    }
}

impl Default for HmgiTierPolicy {
    fn default() -> Self {
        use crate::infrastructure::tier_policy_engine::CollectionStorageConfig;

        let collection_config = CollectionStorageConfig::from_base_location(
            "hmgi_default".to_string(),
            "/mnt/nvme".to_string(),
        )
        .unwrap();
        let available_tiers = vec![
            crate::infrastructure::InfrastructureTier::Memory,
            crate::infrastructure::InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            },
        ];
        let tier_configs = std::collections::HashMap::new();

        let policy = SmartTierPolicy::for_index_workload_constrained(
            collection_config,
            &available_tiers,
            &tier_configs,
        );
        Self::new(policy)
    }
}

/// Result of a tier change operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierChangeResult {
    /// Modality that changed tiers
    pub modality: String,

    /// Previous tier
    pub from_tier: InfrastructureTier,

    /// New tier
    pub to_tier: InfrastructureTier,

    /// Reason for the change
    pub reason: TierChangeReason,
}

/// Reason for tier change
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TierChangeReason {
    /// Manual promotion by administrator
    ManualPromotion,

    /// Manual demotion by administrator
    ManualDemotion,

    /// Automatic promotion based on access patterns
    Promotion,

    /// Automatic demotion based on access patterns
    Demotion,

    /// Scheduled maintenance
    Maintenance,

    /// Cost optimization
    CostOptimization,
}

/// Recommendation for tier change
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierChangeRecommendation {
    /// Modality that should change tiers
    pub modality: String,

    /// Current tier
    pub current_tier: InfrastructureTier,

    /// Recommended tier
    pub recommended_tier: InfrastructureTier,

    /// Reason for the recommendation
    pub reason: TierChangeReason,

    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
}

/// Statistics for tier management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierManagementStats {
    /// Total number of modalities being managed
    pub total_modalities: usize,

    /// Number of modalities in each tier
    pub modality_counts_per_tier: HashMap<String, usize>,

    /// Number of pending tier changes
    pub pending_changes: usize,

    /// Total tier changes performed
    pub total_changes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cloud_provider() -> crate::infrastructure::tier_policy_engine::CloudProvider {
        crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
            bucket: "proximadb-hmgi-test".to_string(),
            storage_class: crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
            lifecycle_enabled: false,
        }
    }

    #[test]
    fn test_tier_selection_from_pattern() {
        let policy = HmgiTierPolicy::default();

        // Hot pattern
        let hot_pattern = AccessPatternMetrics {
            hot_access_rate: 150.0,
            warm_access_rate: 10.0,
            cold_access_rate: 1.0,
            sequential_access_pct: 20.0,
            random_access_pct: 80.0,
        };
        let tier = policy.select_tier_from_pattern(&hot_pattern);
        assert!(matches!(tier, InfrastructureTier::Memory));

        // Warm pattern
        let warm_pattern = AccessPatternMetrics {
            hot_access_rate: 50.0,
            warm_access_rate: 20.0,
            cold_access_rate: 5.0,
            sequential_access_pct: 30.0,
            random_access_pct: 70.0,
        };
        let tier = policy.select_tier_from_pattern(&warm_pattern);
        assert!(matches!(tier, InfrastructureTier::NvmeSsd { .. }));

        // Cold pattern
        let cold_pattern = AccessPatternMetrics {
            hot_access_rate: 0.5,
            warm_access_rate: 2.0,
            cold_access_rate: 10.0,
            sequential_access_pct: 50.0,
            random_access_pct: 50.0,
        };
        let tier = policy.select_tier_from_pattern(&cold_pattern);
        assert!(matches!(tier, InfrastructureTier::CloudStandard { .. }));
    }

    #[tokio::test]
    async fn test_modality_tier_override() {
        let policy = HmgiTierPolicy::default();
        let cloud_tier = InfrastructureTier::CloudStandard {
            provider: test_cloud_provider(),
            region: "us-west-2".to_string(),
        };

        policy
            .set_modality_tier("archive".to_string(), cloud_tier.clone())
            .await;

        let tier = policy.select_tier_for_modality("archive").await;
        assert_eq!(tier, cloud_tier);

        // Default tier for other modalities
        let default_tier = policy.select_tier_for_modality("unknown").await;
        assert!(matches!(default_tier, InfrastructureTier::NvmeSsd { .. }));
    }

    #[tokio::test]
    async fn test_access_tracking() {
        let policy = HmgiTierPolicy::default();

        // Record some accesses
        for _ in 0..10 {
            policy.record_access("text", true).await;
        }
        for _ in 0..5 {
            policy.record_access("text", false).await;
        }

        let pattern = policy.get_access_pattern("text").await;
        assert!(pattern.is_some());
        let pattern = pattern.unwrap();
        assert_eq!(pattern.hot_access_rate, 10.0);
        assert_eq!(pattern.warm_access_rate, 5.0);
    }

    #[tokio::test]
    async fn test_promote_modality() {
        let policy = HmgiTierPolicy::default();

        // Start with NVMe tier (default)
        let initial_tier = policy.select_tier_for_modality("hot_modality").await;
        assert!(matches!(initial_tier, InfrastructureTier::NvmeSsd { .. }));

        // Promote to Memory
        let result = policy
            .promote_modality("hot_modality", InfrastructureTier::Memory)
            .await
            .unwrap();

        assert_eq!(result.modality, "hot_modality");
        assert!(matches!(
            result.from_tier,
            InfrastructureTier::NvmeSsd { .. }
        ));
        assert!(matches!(result.to_tier, InfrastructureTier::Memory));
        assert_eq!(result.reason, TierChangeReason::Promotion);

        // Verify the new tier
        let new_tier = policy.select_tier_for_modality("hot_modality").await;
        assert!(matches!(new_tier, InfrastructureTier::Memory));
    }

    #[tokio::test]
    async fn test_demote_modality() {
        let policy = HmgiTierPolicy::default();

        // Manually set a hot tier first
        policy
            .set_modality_tier("cold_modality".to_string(), InfrastructureTier::Memory)
            .await;

        // Demote to Cloud
        let cloud_tier = InfrastructureTier::CloudStandard {
            provider: test_cloud_provider(),
            region: "us-east-1".to_string(),
        };

        let result = policy
            .demote_modality("cold_modality", cloud_tier.clone())
            .await
            .unwrap();

        assert_eq!(result.modality, "cold_modality");
        assert!(matches!(result.from_tier, InfrastructureTier::Memory));
        assert_eq!(result.to_tier, cloud_tier);
        assert_eq!(result.reason, TierChangeReason::Demotion);
    }

    #[tokio::test]
    async fn test_promote_to_colder_tier_fails() {
        let policy = HmgiTierPolicy::default();

        // Try to "promote" to a colder tier (should fail)
        let cloud_tier = InfrastructureTier::CloudStandard {
            provider: test_cloud_provider(),
            region: "us-east-1".to_string(),
        };

        let result = policy.promote_modality("test", cloud_tier).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_demote_to_hotter_tier_fails() {
        let policy = HmgiTierPolicy::default();

        // Set up a modality in cloud tier
        let cloud_tier = InfrastructureTier::CloudStandard {
            provider: test_cloud_provider(),
            region: "us-east-1".to_string(),
        };
        policy
            .set_modality_tier("test".to_string(), cloud_tier)
            .await;

        // Try to "demote" to a hotter tier (should fail)
        let result = policy
            .demote_modality("test", InfrastructureTier::Memory)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_auto_evaluate_tiers() {
        let policy = HmgiTierPolicy::default();

        // Set up access patterns
        policy.record_access("hot_modality", true).await;
        for _ in 0..100 {
            policy.record_access("hot_modality", true).await;
        }

        for _ in 0..50 {
            policy.record_access("cold_modality", true).await;
        }

        // Reset and re-track to establish pattern
        policy.reset_access_patterns().await;

        // Track hot pattern
        for _ in 0..150 {
            policy.record_access("hot_modality", true).await;
        }

        // Track cold pattern
        for _ in 0..5 {
            policy.record_access("cold_modality", true).await;
        }

        let recommendations = policy.auto_evaluate_tiers().await;

        // Should have recommendations based on access patterns
        assert!(!recommendations.is_empty());
    }
}
