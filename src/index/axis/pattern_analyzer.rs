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

//! AXIS Index Tiering Integration with Existing Infrastructure
//!
//! This module provides AXIS-specific tiering integration that leverages the existing
//! unified pattern analysis and tier management infrastructure instead of duplicating
//! functionality.
//!
//! ## Integration Architecture:
//! - Uses existing AccessPatternTracker from CrossCacheOrchestrator
//! - Leverages existing GlobalTier for tier decisions  
//! - Integrates with existing WorkloadMetrics and AccessPatternMetrics
//! - Extends existing StorageTier enum with AXIS-specific mappings
//! - Reuses existing policy engine infrastructure

use crate::index::axis::integration::collection_state::TierLevel;
use crate::infrastructure::tier_policy_engine::{GlobalTier, InfrastructureTier, WorkloadPattern};
use crate::storage::cache::orchestrator::{AccessPatternTracker, CacheType};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

/// AXIS Tiering Integration Manager
///
/// Integrates AXIS index tiering with existing unified infrastructure instead of
/// duplicating pattern analysis, tier management, and policy engine functionality.
pub struct AxisTieringIntegration {
    /// Reference to existing unified access pattern tracker
    access_tracker: Arc<AccessPatternTracker>,

    /// Reference to existing global tier manager  
    global_tier_manager: Arc<GlobalTier>,

    /// AXIS-specific index type preferences for tier mapping
    index_type_preferences: AxisIndexPreferences,
}

/// AXIS-specific index type preferences for tier optimization
#[derive(Debug, Clone)]
pub struct AxisIndexPreferences {
    /// Index type to tier preferences mapping
    preferences: std::collections::HashMap<AxisIndex, IndexTierPreference>,
}

/// Index-specific tier preference configuration
#[derive(Debug, Clone)]
pub struct IndexTierPreference {
    /// Preferred tier level (1=Memory, 2=NVMe, 3=HDD, etc.)
    preferred_tier_level: u8,

    /// Minimum acceptable tier (won't place slower than this)
    min_tier_level: u8,

    /// Maximum acceptable tier (won't place faster than this unless hot)
    max_tier_level: u8,

    /// Access frequency multiplier for tier decisions
    frequency_multiplier: f64,
}

/// AXIS tier recommendation result  
#[derive(Debug, Clone)]
pub struct AxisTierRecommendation {
    /// Collection identifier
    pub collection_id: String,

    /// Current AXIS tier level
    pub current_tier: TierLevel,

    /// Recommended AXIS tier level
    pub recommended_tier: TierLevel,

    /// Unified storage tier mapping
    pub storage_tier: InfrastructureTier,

    /// Confidence score (0.0-1.0)

    /// Recommendation rationale
    pub rationale: String,
}

/// AXIS index types for tier optimization
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AxisIndex {
    /// Hierarchical Navigable Small World (memory-preferred)
    HNSW,

    /// Inverted File with clustering (NVMe-preferred)
    IVF,

    /// Product Quantization (flexible)
    PQ,

    /// Locality Sensitive Hashing (disk-tolerant)
    LSH,

    /// Flat/brute force index (memory-intensive)
    Flat,

    /// Annoy tree-based index (disk-friendly)
    Annoy,
}

impl Default for AxisIndexPreferences {
    fn default() -> Self {
        let mut preferences = std::collections::HashMap::new();

        // HNSW: High memory preference, sensitive to latency
        preferences.insert(
            AxisIndex::HNSW,
            IndexTierPreference {
                preferred_tier_level: 1, // Memory
                min_tier_level: 1,
                max_tier_level: 2, // Up to NVMe if needed
                frequency_multiplier: 1.5,
            },
        );

        // IVF: Moderate preference, NVMe-optimized
        preferences.insert(
            AxisIndex::IVF,
            IndexTierPreference {
                preferred_tier_level: 2, // NVMe
                min_tier_level: 1,
                max_tier_level: 3, // Up to HDD acceptable
                frequency_multiplier: 1.2,
            },
        );

        // LSH: Disk-tolerant, flexible placement
        preferences.insert(
            AxisIndex::LSH,
            IndexTierPreference {
                preferred_tier_level: 3, // HDD acceptable
                min_tier_level: 1,
                max_tier_level: 4, // Can use cloud tiers
                frequency_multiplier: 1.0,
            },
        );

        // Flat: Memory-intensive, avoid slow tiers
        preferences.insert(
            AxisIndex::Flat,
            IndexTierPreference {
                preferred_tier_level: 1, // Memory
                min_tier_level: 1,
                max_tier_level: 2, // NVMe at most
                frequency_multiplier: 2.0,
            },
        );

        Self { preferences }
    }
}

impl AxisTieringIntegration {
    /// Create new AXIS tiering integration with existing infrastructure
    pub fn new(
        access_tracker: Arc<AccessPatternTracker>,
        global_tier_manager: Arc<GlobalTier>,
    ) -> Self {
        Self {
            access_tracker,
            global_tier_manager,
            index_type_preferences: AxisIndexPreferences::default(),
        }
    }

    /// Track AXIS index access using existing unified infrastructure
    pub async fn track_index_access(&self, collection_id: &str, index_type: AxisIndex) {
        // Use existing unified AccessPatternTracker
        self.access_tracker
            .track_access_async(collection_id.to_string(), CacheType::IndexStructure);

        info!(
            "Tracked AXIS {} index access for collection: {}",
            self.format_index_type(&index_type),
            collection_id
        );
    }

    /// Get tier recommendation using existing unified infrastructure
    pub async fn recommend_tier(
        &self,
        collection_id: &str,
        current_tier: &TierLevel,
        index_type: AxisIndex,
    ) -> Result<Option<AxisTierRecommendation>> {
        // Check if frequently accessed using existing tracker
        let is_hot = self
            .access_tracker
            .is_frequently_accessed(collection_id, 10)
            .await;

        // Get index type preferences
        let preference = self.index_type_preferences.preferences.get(&index_type);

        let recommended_tier = if is_hot {
            // Hot data: use index type's preferred tier or faster
            if let Some(pref) = preference {
                self.map_tier_level_to_axis(pref.preferred_tier_level)
            } else {
                TierLevel::Memory // Default for unknown types
            }
        } else {
            // Cold data: use existing GlobalTier rule-based policy
            let rule_policy = self.global_tier_manager.rule_based_policy();
            let workload_pattern = WorkloadPattern::ReadHeavy; // AXIS indexes are read-heavy
            let tier_level = rule_policy.determine_tier(&workload_pattern, 1.0, 1); // Low frequency
            self.map_tier_level_to_axis(tier_level)
        };

        // Only recommend if different from current tier
        if recommended_tier != *current_tier {
            let storage_tier = self.map_axis_tier_to_storage(&recommended_tier);

            Ok(Some(AxisTierRecommendation {
                collection_id: collection_id.to_string(),
                current_tier: current_tier.clone(),
                recommended_tier,
                storage_tier,
                // confidence removed -  if is_hot { 0.9 } else { 0.7 },
                rationale: self.generate_rationale(&index_type, is_hot, &recommended_tier),
            }))
        } else {
            Ok(None)
        }
    }

    /// Helper methods for tier mapping and utilities
    pub fn map_tier_level_to_axis(&self, tier_level: u8) -> TierLevel {
        match tier_level {
            1 => TierLevel::Memory,
            2 => TierLevel::Disk,
            _ => TierLevel::Cloud,
        }
    }

    pub fn map_axis_tier_to_storage(&self, axis_tier: &TierLevel) -> InfrastructureTier {
        match axis_tier {
            TierLevel::Memory => InfrastructureTier::Memory,
            TierLevel::Disk => InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            },
            TierLevel::Cloud => InfrastructureTier::CloudStandard {
                provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                    bucket: "proximadb-axis-standard".to_string(),
                    storage_class:
                        crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            },
        }
    }

    fn format_index_type(&self, index_type: &AxisIndex) -> &'static str {
        match index_type {
            AxisIndex::HNSW => "HNSW",
            AxisIndex::IVF => "IVF",
            AxisIndex::PQ => "PQ",
            AxisIndex::LSH => "LSH",
            AxisIndex::Flat => "Flat",
            AxisIndex::Annoy => "Annoy",
        }
    }

    fn generate_rationale(&self, index_type: &AxisIndex, is_hot: bool, tier: &TierLevel) -> String {
        let index_name = self.format_index_type(index_type);

        if is_hot {
            format!(
                "{} index with high access frequency recommended for {:?} tier for optimal performance",
                index_name, tier
            )
        } else {
            format!(
                "{} index with low access frequency can use {:?} tier for cost efficiency",
                index_name, tier
            )
        }
    }
}

/// AXIS operation types for pattern analysis  
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxisOperation {
    Search,
    Insert,
    Update,
    Delete,
    Rebuild,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_axis_tiering_integration() {
        let access_tracker = Arc::new(AccessPatternTracker::new(1000));
        let global_manager = Arc::new(GlobalTier::new());
        let integration = AxisTieringIntegration::new(access_tracker, global_manager);

        // Test index access tracking
        integration
            .track_index_access("test_collection", AxisIndex::HNSW)
            .await;

        // Test tier recommendation
        let recommendation = integration
            .recommend_tier("test_collection", &TierLevel::Disk, AxisIndex::HNSW)
            .await
            .unwrap();

        // Should recommend faster tier for HNSW if accessed
        if let Some(rec) = recommendation {
            tracing::debug!(
                "Recommendation: {} -> {:?}",
                rec.collection_id,
                rec.recommended_tier
            );
        }
    }

    #[tokio::test]
    async fn test_tier_mapping() {
        let access_tracker = Arc::new(AccessPatternTracker::new(1000));
        let global_manager = Arc::new(GlobalTier::new());
        let integration = AxisTieringIntegration::new(access_tracker, global_manager);

        // Test tier level mapping
        assert_eq!(integration.map_tier_level_to_axis(1), TierLevel::Memory);
        assert_eq!(integration.map_tier_level_to_axis(2), TierLevel::Disk);
        assert_eq!(integration.map_tier_level_to_axis(3), TierLevel::Cloud);

        // Test storage tier mapping
        let storage_tier = integration.map_axis_tier_to_storage(&TierLevel::Memory);
        assert_eq!(
            storage_tier,
            crate::infrastructure::tier_policy_engine::InfrastructureTier::Memory
        );
    }
}
