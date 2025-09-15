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

//! AXIS Index Tiering Manager - Integrated with Existing Infrastructure
//!
//! This module provides AXIS-specific tiering logic while leveraging the existing
//! infrastructure from GlobalTier, AccessPatternTracker, and AdaptiveStore.
//!
//! ## Integration Architecture
//!
//! ```text
//! AxisTieringManager
//!     ↓
//! ┌──────────────────────────────────────────────────────────────────┐
//! │              Existing Infrastructure                             │
//! │                                                                  │
//! │ GlobalTier ↔ AccessPatternTracker                        │
//! │       ↓                     ↓                                    │
//! │ InfrastructureTier Hierarchy    Pattern Analysis                        │
//! │ (Memory→NVMe→Cloud)      (Heat Scoring)                         │
//! │       ↓                     ↓                                    │
//! │ AdaptiveStore.IndexBackend                                       │
//! │ (DashMap with tier awareness)                                    │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Changes
//!
//! 1. **Reuse AccessPatternTracker**: Instead of custom heat scoring
//! 2. **Leverage GlobalTier**: Instead of custom tier policies
//! 3. **Integrate with AdaptiveStore**: Use IndexBackend for AXIS indexes
//! 4. **Collection-Level Granularity**: Entire collection indexes move together
//! 5. **Format Strategy Integration**: Bincode/Avro selection based on tier

use crate::index::axis::integration::collection_state::{
    CollectionStateManager, CollectionTierState,
};
use crate::index::axis::integration::memory_tracker::IndexMemoryTracker;
use crate::index::axis::storage::format_strategy::{IndexFormatStrategy, IndexSerializationFormat};
use crate::index::axis::storage::serialization::IndexSerializer;
use crate::infrastructure::adaptive_structures::{AdaptiveStore, IndexBackend};
use crate::infrastructure::tier_policy_engine::{
    AccessPatternMetrics, GlobalTier, InfrastructureTier, SmartTierPolicy, WorkloadMetrics,
    WorkloadPattern,
};
use crate::storage::cache::orchestrator::{AccessPatternTracker, CacheType};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// AXIS-specific tiering configuration that integrates with existing policies
#[derive(Debug, Clone)]
pub struct AxisTieringConfig {
    /// Enable automatic tiering (delegates to GlobalTier)
    pub enable_auto_tiering: bool,

    /// Collection-level constraints override
    pub collection_constraints: Option<CollectionTieringConstraints>,

    /// Format strategy preferences per tier
    pub format_preferences: TierFormatPreferences,

    /// Index type specific settings
    pub index_type_settings: IndexSettings,

    /// Integration settings with existing infrastructure
    pub integration_config: IntegrationConfig,
}

/// Collection-specific tiering constraints
#[derive(Debug, Clone)]
pub struct CollectionTieringConstraints {
    /// Collections that must stay in memory
    pub memory_pinned_collections: Vec<String>,

    /// Collections that cannot go to cloud
    pub no_cloud_collections: Vec<String>,

    /// Maximum tier per collection
    pub max_tier_per_collection: std::collections::HashMap<String, InfrastructureTier>,

    /// Custom workload patterns per collection
    pub workload_overrides: std::collections::HashMap<String, WorkloadPattern>,
}

/// Format preferences per storage tier
#[derive(Debug, Clone)]
pub struct TierFormatPreferences {
    /// Hot tier (Memory/NVMe): Fast serialization
    pub hot_tier_format: IndexSerializationFormat,

    /// Warm tier (SSD/HDD): Balanced compression
    pub warm_tier_format: IndexSerializationFormat,

    /// Cold tier (Cloud): Maximum compression
    pub cold_tier_format: IndexSerializationFormat,
}

/// Index type specific settings
#[derive(Debug, Clone)]
pub struct IndexSettings {
    /// HNSW-specific tier preferences
    pub hnsw_preferences: IndexTierPreference,

    /// IVF-specific tier preferences
    pub ivf_preferences: IndexTierPreference,

    /// LSH-specific tier preferences
    pub lsh_preferences: IndexTierPreference,
}

/// Tier preferences for specific index types
#[derive(Debug, Clone)]
pub struct IndexTierPreference {
    /// Preferred tier for this index type
    pub preferred_tier: InfrastructureTier,

    /// Minimum tier (won't go below this)
    pub minimum_tier: InfrastructureTier,

    /// Access pattern boost factor for this index type
    pub access_pattern_boost: f64,
}

/// Configuration for integration with existing infrastructure
#[derive(Debug, Clone)]
pub struct IntegrationConfig {
    /// Use existing AccessPatternTracker for heat scoring
    pub use_existing_pattern_tracker: bool,

    /// Cache type to use for AXIS index access tracking
    pub cache_type_for_tracking: CacheType,

    /// Integration with AdaptiveStore IndexBackend
    pub use_adaptive_store_backend: bool,

    /// Batch size for tier operations
    pub tier_operation_batch_size: usize,

    /// Async operation timeout
    pub operation_timeout: Duration,
}

impl Default for AxisTieringConfig {
    fn default() -> Self {
        Self {
            enable_auto_tiering: true,
            collection_constraints: None,
            format_preferences: TierFormatPreferences {
                hot_tier_format: IndexSerializationFormat::Bincode,
                warm_tier_format: IndexSerializationFormat::BincodeCompressed,
                cold_tier_format: IndexSerializationFormat::AvroZstd,
            },
            index_type_settings: IndexSettings {
                hnsw_preferences: IndexTierPreference {
                    preferred_tier: InfrastructureTier::Memory,
                    minimum_tier: InfrastructureTier::NvmeSsd {
                        mount_path: "/fast".to_string(),
                    },
                    access_pattern_boost: 1.5,
                },
                ivf_preferences: IndexTierPreference {
                    preferred_tier: InfrastructureTier::NvmeSsd {
                        mount_path: "/fast".to_string(),
                    },
                    minimum_tier: InfrastructureTier::HardDisk {
                        mount_path: "/data".to_string(),
                    },
                    access_pattern_boost: 1.2,
                },
                lsh_preferences: IndexTierPreference {
                    preferred_tier: InfrastructureTier::HardDisk {
                        mount_path: "/data".to_string(),
                    },
                    minimum_tier: InfrastructureTier::CloudStandard {
                        provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                            bucket: "proximadb-indexes".to_string(),
                            storage_class:
                                crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
                            lifecycle_enabled: true,
                        },
                        region: "us-west-2".to_string(),
                    },
                    access_pattern_boost: 1.0,
                },
            },
            integration_config: IntegrationConfig {
                use_existing_pattern_tracker: true,
                cache_type_for_tracking: CacheType::IndexStructure,
                use_adaptive_store_backend: true,
                tier_operation_batch_size: 100,
                operation_timeout: Duration::from_secs(300),
            },
        }
    }
}

/// Statistics for AXIS tiering operations
#[derive(Debug, Clone, Default)]
pub struct TieringStats {
    pub promotions: u64,
    pub demotions: u64,
    pub prefetch_hits: u64,
    pub prefetch_misses: u64,
    pub bytes_promoted: u64,
    pub bytes_demoted: u64,
    pub last_evaluation: Option<Instant>,
    pub integration_stats: IntegrationStats,
}

/// Statistics for integration with existing infrastructure
#[derive(Debug, Clone, Default)]
pub struct IntegrationStats {
    pub pattern_tracker_queries: u64,
    pub global_tier_manager_calls: u64,
    pub adaptive_store_operations: u64,
    pub format_conversions: u64,
}

/// AXIS Tiering Manager - Integrated with Existing Infrastructure
pub struct AxisTieringManager {
    /// Configuration
    config: AxisTieringConfig,

    /// Existing infrastructure components
    global_tier_manager: Arc<GlobalTier>,
    access_pattern_tracker: Arc<AccessPatternTracker>,
    index_backend: Arc<IndexBackend<String, Vec<u8>>>,

    /// AXIS-specific components
    collection_state_manager: Arc<CollectionStateManager>,
    memory_tracker: Arc<IndexMemoryTracker>,
    format_strategy: Arc<IndexFormatStrategy>,
    serializer: Arc<IndexSerializer>,

    /// Statistics
    stats: Arc<RwLock<TieringStats>>,

    /// Active tier operations
    active_operations: Arc<DashMap<String, TierOperation>>,
}

/// A tier operation in progress
#[derive(Debug, Clone)]
struct TierOperation {
    collection_id: String,
    from_tier: InfrastructureTier,
    to_tier: InfrastructureTier,
    start_time: Instant,
    operation_type: TierOperationType,
}

/// Type of tier operation
#[derive(Debug, Clone, PartialEq, Eq)]
enum TierOperationType {
    Promotion,
    Demotion,
    Migration,
    Prefetch,
}

impl AxisTieringManager {
    /// Create new AXIS tiering manager with existing infrastructure integration
    pub fn new(
        config: AxisTieringConfig,
        global_tier_manager: Arc<GlobalTier>,
        access_pattern_tracker: Arc<AccessPatternTracker>,
        index_backend: Arc<IndexBackend<String, Vec<u8>>>,
        collection_state_manager: Arc<CollectionStateManager>,
        memory_tracker: Arc<IndexMemoryTracker>,
    ) -> Self {
        let format_strategy = Arc::new(IndexFormatStrategy);
        let serializer = Arc::new(IndexSerializer);

        Self {
            config,
            global_tier_manager,
            access_pattern_tracker,
            index_backend,
            collection_state_manager,
            memory_tracker,
            format_strategy,
            serializer,
            stats: Arc::new(RwLock::new(TieringStats::default())),
            active_operations: Arc::new(DashMap::new()),
        }
    }

    /// Start the tiering manager with periodic evaluation
    pub async fn start(&self) -> tokio::task::JoinHandle<()> {
        let manager = self.clone();
        tokio::spawn(async move {
            manager.run_periodic_evaluation().await;
        })
    }

    /// Main evaluation loop leveraging existing infrastructure
    /// Runs every 5 minutes (300s) to avoid resource exhaustion
    async fn run_periodic_evaluation(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(300));

        loop {
            interval.tick().await;

            if !self.config.enable_auto_tiering {
                continue;
            }

            if let Err(e) = self.evaluate_and_execute_tier_changes().await {
                error!("Error during tier evaluation: {}", e);
            }

            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats.last_evaluation = Some(Instant::now());
            }
        }
    }

    /// Handle memory pressure using TransactionCoordinator
    pub async fn handle_memory_pressure(&self) -> anyhow::Result<()> {
        info!("Handling memory pressure for AXIS indexes");

        // Check current memory pressure
        let memory_stats = self.memory_tracker.memory_stats().await;
        let memory_pressure = memory_stats.memory_usage_percentage / 100.0;

        if memory_pressure < 0.7 {
            debug!(
                "Memory pressure is acceptable at {:.1}%",
                memory_pressure * 100.0
            );
            return Ok(());
        }

        warn!(
            "High memory pressure detected: {:.1}%",
            memory_pressure * 100.0
        );

        // Get collections currently in memory
        let collection_states = self.collection_state_manager.get_all_states().await?;
        let mut memory_collections = Vec::new();

        for (collection_id, state) in collection_states {
            let tier = self.extract_tier_from_state(&state)?;
            if matches!(tier, InfrastructureTier::Memory) {
                memory_collections.push(collection_id);
            }
        }

        // Sort by access frequency (least accessed first)
        let mut collection_frequencies = Vec::new();
        for collection_id in memory_collections {
            let is_hot = self
                .access_pattern_tracker
                .is_frequently_accessed(&collection_id, 10)
                .await;
            collection_frequencies.push((collection_id, if is_hot { 100 } else { 1 }));
        }
        collection_frequencies.sort_by_key(|&(_, freq)| freq);

        // Demote least frequently accessed collections until pressure is relieved
        let target_pressure = 0.6; // Target 60% memory usage
        let mut demoted_count = 0;

        for (collection_id, _frequency) in collection_frequencies {
            // Check if memory-pinned
            if let Some(constraints) = &self.config.collection_constraints {
                if constraints
                    .memory_pinned_collections
                    .contains(&collection_id)
                {
                    debug!("Skipping memory-pinned collection {}", collection_id);
                    continue;
                }
            }

            // Demote to NVMe
            let target_tier = InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            };

            match self.execute_tier_change(&collection_id, target_tier).await {
                Ok(_) => {
                    info!(
                        "Demoted collection {} to relieve memory pressure",
                        collection_id
                    );
                    demoted_count += 1;

                    // Check if pressure is now acceptable
                    let new_stats = self.memory_tracker.memory_stats().await;
                    let new_pressure = new_stats.memory_usage_percentage / 100.0;
                    if new_pressure < target_pressure {
                        info!(
                            "Memory pressure relieved to {:.1}% after demoting {} collections",
                            new_pressure * 100.0,
                            demoted_count
                        );
                        break;
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to demote collection {} for memory pressure: {}",
                        collection_id, e
                    );
                }
            }
        }

        if demoted_count == 0 && memory_pressure > 0.9 {
            error!(
                "CRITICAL: Unable to relieve memory pressure at {:.1}%",
                memory_pressure * 100.0
            );
        }

        Ok(())
    }

    /// Evaluate and execute tier changes using existing infrastructure
    async fn evaluate_and_execute_tier_changes(&self) -> anyhow::Result<()> {
        info!("Starting AXIS tier evaluation using integrated infrastructure");

        // Step 1: Check memory pressure first
        if let Err(e) = self.handle_memory_pressure().await {
            error!("Failed to handle memory pressure: {}", e);
        }

        // Step 2: Auto-promote hot collections
        match self.auto_promote_collections().await {
            Ok(count) => {
                if count > 0 {
                    info!("Auto-promoted {} collections", count);
                }
            }
            Err(e) => error!("Auto-promotion failed: {}", e),
        }

        // Step 3: Auto-demote cold collections
        match self.auto_demote_collections().await {
            Ok(count) => {
                if count > 0 {
                    info!("Auto-demoted {} collections", count);
                }
            }
            Err(e) => error!("Auto-demotion failed: {}", e),
        }

        // Step 4: Get collection states for detailed evaluation
        let collection_states = self.collection_state_manager.get_all_states().await?;

        // Step 5: For each collection, analyze using existing infrastructure
        for (collection_id, current_state) in collection_states {
            // Skip if operation already in progress
            if self.active_operations.contains_key(&collection_id) {
                continue;
            }

            // Get workload metrics from IndexBackend
            let workload_metrics = self.get_collection_workload_metrics(&collection_id).await?;

            // Get access patterns from AccessPatternTracker
            let access_patterns = self.get_collection_access_patterns(&collection_id).await?;

            // Get tier recommendation using existing RuleBasedTierPolicy
            let rule_policy = self.global_tier_manager.rule_based_policy();
            let access_freq = if access_patterns.hot_access_rate > 0.5 {
                100.0
            } else {
                10.0
            };
            let age_days = 7; // Simplified for now
            let tier_level =
                rule_policy.determine_tier(&workload_metrics.pattern, access_freq, age_days);
            let policy_recommendation = self.tier_level_to_storage_tier(tier_level);

            // Apply AXIS-specific logic
            let axis_recommendation = self
                .apply_axis_specific_logic(
                    &collection_id,
                    &current_state,
                    &policy_recommendation,
                    &access_patterns,
                    &workload_metrics,
                )
                .await?;

            // Execute tier change if needed
            if let Some(target_tier) = axis_recommendation {
                self.execute_tier_change(&collection_id, target_tier)
                    .await?;
            }
        }

        info!("Completed AXIS tier evaluation");
        Ok(())
    }

    /// Get workload metrics for a collection from existing infrastructure
    async fn get_collection_workload_metrics(
        &self,
        collection_id: &str,
    ) -> anyhow::Result<WorkloadMetrics> {
        // Query the IndexBackend for workload characteristics
        let workload_metrics = self.index_backend.workload_metrics().await;

        // Apply collection-specific overrides if configured
        let mut metrics = workload_metrics;
        if let Some(constraints) = &self.config.collection_constraints {
            // Check for Mixed workload type override
            for (workload_type, override_pattern) in &constraints.workload_overrides {
                if workload_type == "Mixed" {
                    metrics.pattern = override_pattern.clone();
                    break;
                }
            }
        }

        // Update integration stats
        {
            let mut stats = self.stats.write().await;
            stats.integration_stats.adaptive_store_operations += 1;
        }

        Ok(metrics)
    }

    /// Get access patterns for a collection from AccessPatternTracker
    async fn get_collection_access_patterns(
        &self,
        collection_id: &str,
    ) -> anyhow::Result<AccessPatternMetrics> {
        // Record access tracking query - NOTE: track_access_async is NOT async despite its name
        self.access_pattern_tracker
            .track_access_async(collection_id.to_string(), CacheType::IndexStructure);

        // Get access frequency and pattern data
        // Note: This would need to be extended in AccessPatternTracker to provide this data
        let access_patterns = AccessPatternMetrics {
            hot_access_rate: 0.0,  // Would be calculated from AccessPatternTracker data
            warm_access_rate: 0.0, // Would be calculated from AccessPatternTracker data
            cold_access_rate: 0.0, // Would be calculated from AccessPatternTracker data
            sequential_access_pct: 50.0,
            random_access_pct: 50.0,
        };

        // Update integration stats
        {
            let mut stats = self.stats.write().await;
            stats.integration_stats.pattern_tracker_queries += 1;
        }

        Ok(access_patterns)
    }

    /// Apply AXIS-specific logic on top of global tier recommendations
    async fn apply_axis_specific_logic(
        &self,
        collection_id: &str,
        current_state: &CollectionTierState,
        global_recommendation: &InfrastructureTier,
        _access_patterns: &AccessPatternMetrics,
        workload_metrics: &WorkloadMetrics,
    ) -> anyhow::Result<Option<InfrastructureTier>> {
        // Check collection-specific constraints
        if let Some(constraints) = &self.config.collection_constraints {
            // Memory pinned collections
            if constraints
                .memory_pinned_collections
                .contains(&collection_id.to_string())
            {
                if !matches!(global_recommendation, InfrastructureTier::Memory) {
                    debug!(
                        "Collection {} is memory-pinned, keeping in mem",
                        collection_id
                    );
                    return Ok(Some(InfrastructureTier::Memory));
                }
            }

            // No-cloud collections
            if constraints
                .no_cloud_collections
                .contains(&collection_id.to_string())
            {
                if matches!(
                    global_recommendation,
                    InfrastructureTier::CloudStandard { .. }
                        | InfrastructureTier::CloudInfrequentAccess { .. }
                        | InfrastructureTier::CloudArchive { .. }
                ) {
                    debug!(
                        "Collection {} cannot go to cloud, using HDD instead",
                        collection_id
                    );
                    return Ok(Some(InfrastructureTier::HardDisk {
                        mount_path: "/data".to_string(),
                    }));
                }
            }

            // Maximum tier per collection
            if let Some(max_tier) = constraints.max_tier_per_collection.get(collection_id) {
                if self.tier_order(global_recommendation) > self.tier_order(max_tier) {
                    debug!(
                        "Collection {} limited to tier {:?}",
                        collection_id, max_tier
                    );
                    return Ok(Some(max_tier.clone()));
                }
            }
        }

        // Apply index type specific preferences
        let index_type_adjustment = self
            .apply_index_type_preferences(collection_id, global_recommendation, workload_metrics)
            .await?;

        if let Some(adjusted_tier) = index_type_adjustment {
            return Ok(Some(adjusted_tier));
        }

        // Check if the current state is already optimal
        let current_tier = self.extract_tier_from_state(current_state)?;
        if current_tier == *global_recommendation {
            return Ok(None); // No change needed
        }

        Ok(Some(global_recommendation.clone()))
    }

    /// Apply index type specific preferences
    async fn apply_index_type_preferences(
        &self,
        collection_id: &str,
        global_recommendation: &InfrastructureTier,
        workload_metrics: &WorkloadMetrics,
    ) -> anyhow::Result<Option<InfrastructureTier>> {
        // Use workload pattern as a proxy for index type
        let preference = match workload_metrics.pattern {
            WorkloadPattern::ReadHeavy => &self.config.index_type_settings.hnsw_preferences,
            WorkloadPattern::WriteHeavy => &self.config.index_type_settings.lsh_preferences,
            WorkloadPattern::Mixed | WorkloadPattern::Bulk => {
                &self.config.index_type_settings.ivf_preferences
            }
        };

        // Check if global recommendation violates minimum tier
        if self.tier_order(global_recommendation) > self.tier_order(&preference.minimum_tier) {
            return Ok(Some(preference.minimum_tier.clone()));
        }

        // If global recommendation is worse than preferred, consider using preferred
        if self.tier_order(global_recommendation) > self.tier_order(&preference.preferred_tier) {
            // Apply access pattern boost
            let boosted_frequency =
                workload_metrics.avg_access_frequency * preference.access_pattern_boost;
            if boosted_frequency > 10.0 {
                // Threshold for using preferred tier
                return Ok(Some(preference.preferred_tier.clone()));
            }
        }

        Ok(None)
    }

    /// Auto-promote collections based on access patterns
    pub async fn auto_promote_collections(&self) -> anyhow::Result<u32> {
        info!("Starting auto-promotion evaluation");
        let mut promoted_count = 0;

        // Get all collections eligible for promotion
        let collection_states = self.collection_state_manager.get_all_states().await?;

        for (collection_id, current_state) in collection_states {
            // Check if frequently accessed using existing AccessPatternTracker
            let is_hot = self
                .access_pattern_tracker
                .is_frequently_accessed(&collection_id, 10)
                .await;

            if !is_hot {
                continue; // Skip if not hot
            }

            // Get current tier
            let current_tier = self.extract_tier_from_state(&current_state)?;

            // Check if already at fastest tier
            if matches!(current_tier, InfrastructureTier::Memory) {
                continue;
            }

            // Determine promotion target (one tier faster)
            let target_tier = self.get_promotion_target(&current_tier)?;

            // Check collection constraints
            if !self
                .is_promotion_allowed(&collection_id, &target_tier)
                .await?
            {
                debug!("Promotion blocked by constraints for {}", collection_id);
                continue;
            }

            // Check memory pressure before promoting to memory
            if matches!(target_tier, InfrastructureTier::Memory) {
                let memory_stats = self.memory_tracker.memory_stats().await;
                let memory_pressure = memory_stats.memory_usage_percentage / 100.0;
                if memory_pressure > 0.8 {
                    warn!(
                        "Memory pressure too high ({:.1}%), skipping promotion",
                        memory_pressure * 100.0
                    );
                    continue;
                }
            }

            // Execute promotion
            match self.execute_tier_change(&collection_id, target_tier).await {
                Ok(_) => {
                    info!("Successfully promoted collection {}", collection_id);
                    promoted_count += 1;
                }
                Err(e) => {
                    error!("Failed to promote collection {}: {}", collection_id, e);
                }
            }
        }

        Ok(promoted_count)
    }

    /// Auto-demote collections based on access patterns and constraints
    pub async fn auto_demote_collections(&self) -> anyhow::Result<u32> {
        info!("Starting auto-demotion evaluation");
        let mut demoted_count = 0;

        // Get all collection states
        let collection_states = self.collection_state_manager.get_all_states().await?;

        for (collection_id, current_state) in collection_states {
            // Check if collection is cold (not frequently accessed)
            let is_cold = !self
                .access_pattern_tracker
                .is_frequently_accessed(&collection_id, 3)
                .await;

            if !is_cold {
                continue; // Skip if still warm/hot
            }

            // Get current tier
            let current_tier = self.extract_tier_from_state(&current_state)?;

            // Check if already at slowest allowed tier
            if self
                .is_at_slowest_allowed_tier(&collection_id, &current_tier)
                .await?
            {
                continue;
            }

            // Determine demotion target (one tier slower)
            let target_tier = self.get_demotion_target(&current_tier)?;

            // Check collection constraints
            if !self
                .is_demotion_allowed(&collection_id, &target_tier)
                .await?
            {
                debug!("Demotion blocked by constraints for {}", collection_id);
                continue;
            }

            // Execute demotion
            match self.execute_tier_change(&collection_id, target_tier).await {
                Ok(_) => {
                    info!("Successfully demoted collection {}", collection_id);
                    demoted_count += 1;
                }
                Err(e) => {
                    error!("Failed to demote collection {}: {}", collection_id, e);
                }
            }
        }

        Ok(demoted_count)
    }

    /// Get promotion target (one tier faster)
    fn get_promotion_target(
        &self,
        current_tier: &InfrastructureTier,
    ) -> anyhow::Result<InfrastructureTier> {
        match current_tier {
            InfrastructureTier::CloudArchive { .. }
            | InfrastructureTier::CloudDeepArchive { .. } => {
                Ok(InfrastructureTier::CloudStandard {
                    provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                        bucket: "proximadb-promoted".to_string(),
                        storage_class:
                            crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
                        lifecycle_enabled: true,
                    },
                    region: "us-east-1".to_string(),
                })
            }
            InfrastructureTier::CloudStandard { .. }
            | InfrastructureTier::CloudInfrequentAccess { .. } => {
                Ok(InfrastructureTier::HardDisk {
                    mount_path: "/data".to_string(),
                })
            }
            InfrastructureTier::HardDisk { .. } => Ok(InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            }),
            InfrastructureTier::NvmeSsd { .. } => Ok(InfrastructureTier::Memory),
            InfrastructureTier::Memory => Err(anyhow::anyhow!("Already at fastest tier")),
            _ => {
                Ok(InfrastructureTier::Memory) // Default to memory
            }
        }
    }

    /// Get demotion target (one tier slower)
    fn get_demotion_target(
        &self,
        current_tier: &InfrastructureTier,
    ) -> anyhow::Result<InfrastructureTier> {
        match current_tier {
            InfrastructureTier::Memory => Ok(InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            }),
            InfrastructureTier::NvmeSsd { .. } => Ok(InfrastructureTier::HardDisk {
                mount_path: "/data".to_string(),
            }),
            InfrastructureTier::HardDisk { .. } => Ok(InfrastructureTier::CloudStandard {
                provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                    bucket: "proximadb-demoted".to_string(),
                    storage_class:
                        crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
                    lifecycle_enabled: true,
                },
                region: "us-east-1".to_string(),
            }),
            InfrastructureTier::CloudStandard { .. } => {
                Ok(InfrastructureTier::CloudInfrequentAccess {
                    provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                        bucket: "proximadb-cold".to_string(),
                        storage_class:
                            crate::infrastructure::tier_policy_engine::AwsStorageClass::StandardIA,
                        lifecycle_enabled: true,
                    },
                    region: "us-east-1".to_string(),
                })
            }
            _ => Err(anyhow::anyhow!("Already at slowest tier")),
        }
    }

    /// Check if promotion is allowed based on constraints
    async fn is_promotion_allowed(
        &self,
        collection_id: &str,
        target_tier: &InfrastructureTier,
    ) -> anyhow::Result<bool> {
        if let Some(constraints) = &self.config.collection_constraints {
            // Check max tier constraint
            if let Some(max_tier) = constraints.max_tier_per_collection.get(collection_id) {
                if self.tier_order(target_tier) < self.tier_order(max_tier) {
                    return Ok(false); // Would exceed max tier
                }
            }
        }
        Ok(true)
    }

    /// Check if demotion is allowed based on constraints
    async fn is_demotion_allowed(
        &self,
        collection_id: &str,
        target_tier: &InfrastructureTier,
    ) -> anyhow::Result<bool> {
        if let Some(constraints) = &self.config.collection_constraints {
            // Memory pinned collections cannot be demoted from memory
            if constraints
                .memory_pinned_collections
                .contains(&collection_id.to_string())
            {
                if !matches!(target_tier, InfrastructureTier::Memory) {
                    return Ok(false);
                }
            }

            // No-cloud collections cannot be demoted to cloud
            if constraints
                .no_cloud_collections
                .contains(&collection_id.to_string())
            {
                if matches!(
                    target_tier,
                    InfrastructureTier::CloudStandard { .. }
                        | InfrastructureTier::CloudInfrequentAccess { .. }
                        | InfrastructureTier::CloudArchive { .. }
                        | InfrastructureTier::CloudDeepArchive { .. }
                ) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Check if collection is at slowest allowed tier
    async fn is_at_slowest_allowed_tier(
        &self,
        collection_id: &str,
        current_tier: &InfrastructureTier,
    ) -> anyhow::Result<bool> {
        if let Some(constraints) = &self.config.collection_constraints {
            // No-cloud collections: HDD is slowest
            if constraints
                .no_cloud_collections
                .contains(&collection_id.to_string())
            {
                return Ok(matches!(current_tier, InfrastructureTier::HardDisk { .. }));
            }
        }

        // Default: CloudArchive is slowest we typically use
        Ok(matches!(
            current_tier,
            InfrastructureTier::CloudArchive { .. } | InfrastructureTier::CloudDeepArchive { .. }
        ))
    }

    /// Execute a tier change operation
    async fn execute_tier_change(
        &self,
        collection_id: &str,
        target_tier: InfrastructureTier,
    ) -> anyhow::Result<()> {
        let current_state = self
            .collection_state_manager
            .get_state(collection_id)
            .ok_or_else(|| anyhow::anyhow!("Collection {} not found", collection_id))?;
        let current_tier = self.extract_tier_from_state(&current_state)?;

        info!(
            "Executing tier change for collection {}: {:?} -> {:?}",
            collection_id, current_tier, target_tier
        );

        // Record operation start
        let operation = TierOperation {
            collection_id: collection_id.to_string(),
            from_tier: current_tier.clone(),
            to_tier: target_tier.clone(),
            start_time: Instant::now(),
            operation_type: if self.tier_order(&target_tier) < self.tier_order(&current_tier) {
                TierOperationType::Promotion
            } else {
                TierOperationType::Demotion
            },
        };

        self.active_operations
            .insert(collection_id.to_string(), operation.clone());

        // Choose format based on target tier
        let target_format = self.choose_format_for_tier(&target_tier);

        // Execute the actual tier change
        // Note: GlobalTier doesn't have execute_tier_change, so we handle it here
        let result = self
            .perform_tier_transition(collection_id, &current_tier, &target_tier)
            .await;

        // Handle format conversion if needed
        if result.is_ok() {
            self.handle_format_conversion(collection_id, &target_tier, &target_format)
                .await?;
        }

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            match operation.operation_type {
                TierOperationType::Promotion => stats.promotions += 1,
                TierOperationType::Demotion => stats.demotions += 1,
                _ => {}
            }
            stats.integration_stats.global_tier_manager_calls += 1;
        }

        // Remove from active operations
        self.active_operations.remove(collection_id);

        result.map_err(|e| anyhow::anyhow!("Tier change failed: {}", e))
    }

    /// Perform the actual tier transition using existing infrastructure
    async fn perform_tier_transition(
        &self,
        collection_id: &str,
        _current_tier: &InfrastructureTier,
        target_tier: &InfrastructureTier,
    ) -> anyhow::Result<()> {
        // Use GlobalTier's rebalance_collection_tiers (the actual existing method)
        // Create a SmartTierPolicy for index workload
        let tier_policy = SmartTierPolicy::for_hybrid_workload();
        let rebalance_result = self
            .global_tier_manager
            .rebalance_collection_tiers(collection_id, &tier_policy)
            .await?;

        debug!(
            "Rebalance result for {}: promoted={}, demoted={}, duration={:?}",
            collection_id,
            rebalance_result.promoted_count,
            rebalance_result.demoted_count,
            rebalance_result.duration
        );

        // Update collection state to reflect the tier change
        match target_tier {
            InfrastructureTier::Memory => {
                self.collection_state_manager
                    .transition_to_memory(collection_id)
                    .await?;
            }
            InfrastructureTier::NvmeSsd { mount_path }
            | InfrastructureTier::HardDisk { mount_path } => {
                self.collection_state_manager
                    .transition_to_disk(collection_id, mount_path.clone())
                    .await?;
            }
            _ => {
                // Other tier types not yet implemented
                return Err(anyhow::anyhow!("Unsupported tier type"));
            }
        }

        Ok(())
    }

    /// Choose appropriate format for a storage tier
    fn choose_format_for_tier(&self, tier: &InfrastructureTier) -> IndexSerializationFormat {
        match tier {
            InfrastructureTier::Memory | InfrastructureTier::NvmeSsd { .. } => {
                self.config.format_preferences.hot_tier_format
            }
            InfrastructureTier::HardDisk { .. } => self.config.format_preferences.warm_tier_format,
            _ => self.config.format_preferences.cold_tier_format,
        }
    }

    /// Handle format conversion during tier changes
    async fn handle_format_conversion(
        &self,
        collection_id: &str,
        _target_tier: &InfrastructureTier,
        target_format: &IndexSerializationFormat,
    ) -> anyhow::Result<()> {
        debug!(
            "Handling format conversion for {} to {:?}",
            collection_id, target_format
        );

        // Update integration stats
        {
            let mut stats = self.stats.write().await;
            stats.integration_stats.format_conversions += 1;
        }

        Ok(())
    }

    /// Extract tier from collection state
    fn extract_tier_from_state(
        &self,
        state: &CollectionTierState,
    ) -> anyhow::Result<InfrastructureTier> {
        match state {
            CollectionTierState::Memory { .. } => Ok(InfrastructureTier::Memory),
            CollectionTierState::Disk { disk_location, .. } => {
                // Determine tier based on disk location
                if disk_location.to_string_lossy().contains("nvme") {
                    Ok(InfrastructureTier::NvmeSsd {
                        mount_path: disk_location.to_string_lossy().to_string(),
                    })
                } else {
                    Ok(InfrastructureTier::HardDisk {
                        mount_path: disk_location.to_string_lossy().to_string(),
                    })
                }
            }
            CollectionTierState::Cloud { storage_type, .. } => {
                // Map cloud storage type to tier
                use crate::index::axis::integration::collection_state::CloudStorageType;
                match storage_type {
                    CloudStorageType::S3Standard | CloudStorageType::S3Express => Ok(InfrastructureTier::CloudStandard {
                        provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                            bucket: "proximadb-indexes".to_string(),
                            storage_class: crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
                            lifecycle_enabled: true,
                        },
                        region: "us-west-2".to_string()
                    }),
                    CloudStorageType::S3Glacier => Ok(InfrastructureTier::CloudArchive {
                        provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                            bucket: "proximadb-indexes".to_string(),
                            storage_class: crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
                            lifecycle_enabled: true,
                        },
                        region: "us-west-2".to_string()
                    }),
                    CloudStorageType::GCSStandard | CloudStorageType::GCSNearline | CloudStorageType::GCSArchive => {
                        Ok(InfrastructureTier::CloudStandard {
                            provider: crate::infrastructure::tier_policy_engine::CloudProvider::GoogleCloud {
                                bucket: "proximadb-indexes".to_string(),
                                storage_class: crate::infrastructure::tier_policy_engine::GcsStorageClass::Standard,
                                auto_class: false,
                            },
                            region: "us-central1".to_string()
                        })
                    },
                    CloudStorageType::AzureHot | CloudStorageType::AzureCool | CloudStorageType::AzureArchive => {
                        Ok(InfrastructureTier::CloudStandard {
                            provider: crate::infrastructure::tier_policy_engine::CloudProvider::AzureBlob {
                                account: "proximadb".to_string(),
                                container: "indexes".to_string(),
                                access_tier: crate::infrastructure::tier_policy_engine::AzureAccessTier::Hot,
                            },
                            region: "eastus".to_string()
                        })
                    },
                }
            }
            _ => Err(anyhow::anyhow!(
                "Cannot extract tier from state: {:?}",
                state
            )),
        }
    }

    /// Get tier ordering for comparison (lower number = faster tier)
    fn tier_order(&self, tier: &InfrastructureTier) -> u8 {
        match tier {
            InfrastructureTier::Memory => 0,
            InfrastructureTier::NvmeSsd { .. } => 1,
            InfrastructureTier::HardDisk { .. } => 2,
            InfrastructureTier::CloudExpressOneZone { .. } => 3,
            InfrastructureTier::CloudStandard { .. } => 4,
            InfrastructureTier::CloudInfrequentAccess { .. } => 5,
            InfrastructureTier::CloudArchive { .. } => 6,
            InfrastructureTier::CloudDeepArchive { .. } => 7,
        }
    }

    /// Get current statistics
    pub async fn get_stats(&self) -> TieringStats {
        self.stats.read().await.clone()
    }

    /// Get active operations
    pub fn get_active_operations(&self) -> Vec<TierOperation> {
        self.active_operations
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }
}

// Clone implementation for background task
impl Clone for AxisTieringManager {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            global_tier_manager: Arc::clone(&self.global_tier_manager),
            access_pattern_tracker: Arc::clone(&self.access_pattern_tracker),
            index_backend: Arc::clone(&self.index_backend),
            collection_state_manager: Arc::clone(&self.collection_state_manager),
            memory_tracker: Arc::clone(&self.memory_tracker),
            format_strategy: Arc::clone(&self.format_strategy),
            serializer: Arc::clone(&self.serializer),
            stats: Arc::clone(&self.stats),
            active_operations: Arc::clone(&self.active_operations),
        }
    }
}

impl AxisTieringManager {
    /// Convert tier level (1-4) to InfrastructureTier enum
    fn tier_level_to_storage_tier(&self, tier_level: u8) -> InfrastructureTier {
        match tier_level {
            1 => InfrastructureTier::Memory,
            2 => InfrastructureTier::NvmeSsd {
                mount_path: "/mnt/nvme".to_string(),
            },
            3 => InfrastructureTier::HardDisk {
                mount_path: "/mnt/hdd".to_string(),
            },
            _ => InfrastructureTier::CloudStandard {
                provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                    bucket: "proximadb-indexes".to_string(),
                    storage_class:
                        crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
                    lifecycle_enabled: true,
                },
                region: "us-west-2".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::axis::*;
    use std::sync::Arc;

    #[test]
    fn test_axis_tiering_integration() {
        // Verify construction works
        let config = AxisTieringConfig::default();
        assert!(config.integration_config.use_existing_pattern_tracker);
        assert!(config.integration_config.use_adaptive_store_backend);
    }

    #[tokio::test]
    async fn test_tier_ordering() {
        // Create the necessary components with proper constructors
        let global_tier_manager = Arc::new(GlobalTier::new());
        let access_pattern_tracker = Arc::new(AccessPatternTracker::new(1000));

        // Create a proper IndexBackend using the same pattern as adaptive_structures tests
        let tier_manager = Arc::new({
            let global_tier = Arc::new(GlobalTier::new());
            crate::infrastructure::adaptive_structures::UniversalTier::new(global_tier).await.unwrap()
        });
        let index_backend = Arc::new(
            crate::infrastructure::adaptive_structures::IndexBackend::<String, Vec<u8>>::new_dashmap(
                "test_tiering".to_string(),
                1000,
                None,
                create_test_unified_tier_policy(),
                create_test_adaptive_store_config(),
                tier_manager,
            ).await.unwrap()
        );

        let collection_state_manager = Arc::new(CollectionStateManager::new());
        let memory_tracker = Arc::new(IndexMemoryTracker::new(1.0)); // 1 GB max

        let config = AxisTieringConfig::default();
        let tiering_manager = AxisTieringManager::new(
            config,
            global_tier_manager,
            access_pattern_tracker,
            index_backend,
            collection_state_manager,
            memory_tracker,
        );

        let memory = InfrastructureTier::Memory;
        let nvme = InfrastructureTier::NvmeSsd { mount_path: "/fast".to_string() };
        let cloud = InfrastructureTier::CloudStandard {
            provider: crate::infrastructure::tier_policy_engine::CloudProvider::AwsS3 {
                bucket: "proximadb-indexes".to_string(),
                storage_class: crate::infrastructure::tier_policy_engine::AwsStorageClass::Standard,
                lifecycle_enabled: true,
            },
            region: "us-west-2".to_string()
        };

        assert!(tiering_manager.tier_order(&memory) < tiering_manager.tier_order(&nvme));
        assert!(tiering_manager.tier_order(&nvme) < tiering_manager.tier_order(&cloud));
    }

    // Helper functions for the test
    fn create_test_unified_tier_policy() -> crate::infrastructure::adaptive_structures::UnifiedTierPolicy {
        crate::infrastructure::adaptive_structures::UnifiedTierPolicy {
            eviction_policy: crate::infrastructure::adaptive_structures::EvictionPolicy::SizeBased { max_memory_mb: 100 },
            promotion_criteria: crate::infrastructure::adaptive_structures::PromotionCriteria {
                min_access_frequency: 10,
                frequency_window: std::time::Duration::from_secs(60),
                min_promotion_tier: InfrastructureTier::Memory,
            },
            demotion_criteria: crate::infrastructure::adaptive_structures::DemotionCriteria {
                max_idle_time: std::time::Duration::from_secs(300),
                memory_pressure_threshold: 0.8,
                min_tier: InfrastructureTier::NvmeSsd {
                    mount_path: "/tmp/nvme".to_string(),
                },
            },
            reload_strategy: crate::infrastructure::adaptive_structures::ReloadStrategy {
                load_on_startup: false,
                prefetch_hot_data: false,
                max_initial_load: 0,
                axis_storage_path: "/tmp/test/indexes/".to_string(),
            },
        }
    }

    fn create_test_adaptive_store_config() -> crate::infrastructure::adaptive_structures::AdaptiveStoreConfig {
        crate::infrastructure::adaptive_structures::AdaptiveStoreConfig {
            collection_id: "test_tiering".to_string(),
            backend_type: crate::infrastructure::adaptive_structures::BackendType::Index {
                structure: crate::infrastructure::adaptive_structures::IndexStructure::DashMap {
                    initial_capacity: 1000,
                    memory_limit_mb: None,
                },
                tier_policy: create_test_unified_tier_policy(),
            },
            tier_config: crate::infrastructure::adaptive_structures::TierConfig {
                enable_tiering: true,
                rebalance_interval: std::time::Duration::from_secs(60),
                memory_pressure_threshold: 0.8,
                max_concurrent_operations: 2,
            },
            metrics_config: crate::infrastructure::adaptive_structures::MetricsConfig {
                enable_workload_metrics: true,
                collection_interval: std::time::Duration::from_secs(30),
                history_retention: std::time::Duration::from_secs(300),
            },
        }
    }
}
