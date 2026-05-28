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

//! SST Engine Tiering Integration
//!
//! This module provides integration between the SST storage engine and the tiering policy engine.
//! It enables automatic data movement between storage tiers based on access patterns and policies.
//!
//! ## Integration Points
//!
//! 1. **Read Path**: Records access events for tiering decisions
//! 2. **Flush Path**: Determines target tier for newly flushed data
//! 3. **Compaction Path**: Evaluates tier migration during compaction
//! 4. **Background Migration**: Moves data between tiers asynchronously
//!
//! ## Configuration
//!
//! Tiering is opt-in and disabled by default. Enable via configuration:
//!
//! ```toml
//! [storage.sst.tiering]
//! enabled = true
//! evaluation_interval_secs = 300  # 5 minutes
//! max_concurrent_migrations = 4
//! hot_tier_path = "file:///data/hot"
//! warm_tier_path = "file:///data/warm"
//! cold_tier_path = "s3://bucket/cold"
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::engines::sst::tiering_integration::SstTieringIntegration;
//!
//! // Create tiering integration
//! let tiering = SstTieringIntegration::new(config)?;
//!
//! // Record access event (called from search path)
//! tiering.record_access("collection", "vector_id", AccessType::Read, 1024).await;
//!
//! // Determine flush target tier (called from flush path)
//! let tier = tiering.determine_flush_tier("collection", data_size).await;
//!
//! // Check if migration needed (called from compaction path)
//! let migrations = tiering.evaluate_collection("collection").await?;
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::storage::tiering::{
    AccessEvent, MigrationResult, MigrationTask, PerformanceTier, TieringEngineConfig,
    TieringPolicy, TieringPolicyEngine, TieringStats,
};
// Import additional types from submodules that aren't re-exported at top level
use crate::storage::tiering::policy::TieringMetadata;
use crate::storage::tiering::tracker::AccessType;

/// Configuration for SST tiering integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstTieringConfig {
    /// Enable tiering integration (default: false)
    pub enabled: bool,

    /// Evaluation interval for policies (default: 300 seconds)
    pub evaluation_interval_secs: u64,

    /// Maximum concurrent migrations (default: 4)
    pub max_concurrent_migrations: usize,

    /// Enable automatic policy evaluation (default: true)
    pub auto_evaluate: bool,

    /// Minimum time between migrations for same item (default: 3600 seconds)
    pub migration_cooldown_secs: u64,

    /// Path for hot tier storage (default: primary storage path)
    pub hot_tier_path: Option<String>,

    /// Path for warm tier storage (default: same as hot)
    pub warm_tier_path: Option<String>,

    /// Path for cold tier storage (default: same as warm)
    pub cold_tier_path: Option<String>,

    /// Path for archive tier storage (default: none)
    pub archive_tier_path: Option<String>,

    /// Default tier for new data (default: Warm)
    pub default_tier: PerformanceTier,

    /// Age threshold for cold tier demotion (default: 7 days)
    pub cold_age_threshold_days: u64,

    /// Age threshold for archive tier demotion (default: 30 days)
    pub archive_age_threshold_days: u64,

    /// Access count threshold for hot tier promotion (default: 100)
    pub hot_access_threshold: u64,
}

impl Default for SstTieringConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            evaluation_interval_secs: 300,
            max_concurrent_migrations: 4,
            auto_evaluate: true,
            migration_cooldown_secs: 3600,
            hot_tier_path: None,
            warm_tier_path: None,
            cold_tier_path: None,
            archive_tier_path: None,
            default_tier: PerformanceTier::Warm,
            cold_age_threshold_days: 7,
            archive_age_threshold_days: 30,
            hot_access_threshold: 100,
        }
    }
}

/// SST engine tiering integration
///
/// Provides hooks for the SST engine to integrate with the tiering policy engine.
/// This is designed to be a lightweight wrapper that can be optionally enabled.
pub struct SstTieringIntegration {
    /// Configuration
    config: SstTieringConfig,

    /// Tiering policy engine
    engine: TieringPolicyEngine,

    /// Running state
    running: Arc<RwLock<bool>>,

    /// Start time for uptime tracking
    started_at: Option<Instant>,

    /// Phase 6 data-plane wiring: optional pin registry. When set,
    /// `determine_flush_tier` and `evaluate_collection` consult it
    /// before applying the access-pattern policy, so operator pins
    /// override automatic tier decisions. When unset, behaviour is
    /// the legacy access-pattern-only path.
    pin_registry: Option<Arc<crate::storage::collection_pinning::CollectionPinRegistry>>,
}

impl SstTieringIntegration {
    /// Create a new SST tiering integration
    pub fn new(config: SstTieringConfig) -> Result<Self> {
        let engine_config = TieringEngineConfig {
            evaluation_interval: Duration::from_secs(config.evaluation_interval_secs),
            max_concurrent_migrations: config.max_concurrent_migrations,
            auto_evaluate: config.auto_evaluate,
            migration_cooldown: Duration::from_secs(config.migration_cooldown_secs),
            track_access: true,
        };

        let engine = TieringPolicyEngine::new(engine_config);

        Ok(Self {
            config,
            engine,
            running: Arc::new(RwLock::new(false)),
            started_at: None,
            pin_registry: None,
        })
    }

    /// Phase 6: attach the per-collection pin registry. When set,
    /// flush-tier selection and migration evaluation defer to the
    /// operator's pin instead of the access-pattern policy. Wired by
    /// `SharedServices::new` so the same `Arc` reaches REST handlers
    /// (control plane) and this integration (data plane).
    pub fn with_pin_registry(
        mut self,
        registry: Arc<crate::storage::collection_pinning::CollectionPinRegistry>,
    ) -> Self {
        self.pin_registry = Some(registry);
        self
    }

    /// True when [`Self::with_pin_registry`] supplied a registry. Used
    /// by tests/operators to confirm the data-plane wiring.
    pub fn pin_registry_configured(&self) -> bool {
        self.pin_registry.is_some()
    }

    /// Builder: attach a migration executor so the background eval
    /// loop dispatches generated `MigrationTask`s to physical byte
    /// movement. Without this, the engine produces tasks but no bytes
    /// move (planning-only mode — useful for dry-runs and tests).
    ///
    /// Pair with `TierMigrationExecutor::from_tiering_config` so the
    /// per-tier paths come from the same config block.
    pub fn with_executor(
        mut self,
        executor: Arc<crate::storage::tiering::TierMigrationExecutor>,
    ) -> Self {
        self.engine.set_executor(executor);
        self
    }

    /// Create a new integration with default configuration
    pub fn new_default() -> Result<Self> {
        Self::new(SstTieringConfig::default())
    }

    /// Check if tiering is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Start the tiering integration
    ///
    /// This starts the background evaluation loop and initializes default policies.
    pub async fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            debug!("SST tiering integration is disabled, skipping start");
            return Ok(());
        }

        let mut running = self.running.write().await;
        if *running {
            return Ok(());
        }
        *running = true;
        drop(running);

        // Add default policies based on configuration
        self.add_default_policies().await;

        // Start the tiering engine
        self.engine.start().await?;
        self.started_at = Some(Instant::now());

        info!(
            "SST tiering integration started with {}s evaluation interval",
            self.config.evaluation_interval_secs
        );

        Ok(())
    }

    /// Stop the tiering integration
    pub async fn stop(&mut self) {
        let mut running = self.running.write().await;
        *running = false;

        self.engine.stop().await;
        info!("SST tiering integration stopped");
    }

    /// Add default tiering policies based on configuration
    async fn add_default_policies(&self) {
        // Policy 1: Demote to cold after configured days
        let cold_policy = TieringPolicy::age_based(
            "sst-cold-after-days",
            Duration::from_secs(self.config.cold_age_threshold_days * 24 * 3600),
            PerformanceTier::Cold,
        )
        .with_description("Demote data to cold tier after configured age threshold")
        .with_priority(10);

        self.engine.add_policy(cold_policy).await;

        // Policy 2: Demote to archive after configured days
        if self.config.archive_tier_path.is_some() {
            let archive_policy = TieringPolicy::age_based(
                "sst-archive-after-days",
                Duration::from_secs(self.config.archive_age_threshold_days * 24 * 3600),
                PerformanceTier::Archive,
            )
            .with_description("Archive data after configured age threshold")
            .with_priority(5);

            self.engine.add_policy(archive_policy).await;
        }

        // Policy 3: Promote frequently accessed data to hot tier
        let hot_policy = TieringPolicy::access_based(
            "sst-hot-frequently-accessed",
            self.config.hot_access_threshold,
            PerformanceTier::Hot,
        )
        .with_description("Promote frequently accessed data to hot tier")
        .with_priority(20);

        self.engine.add_policy(hot_policy).await;

        info!(
            "Added {} default tiering policies",
            if self.config.archive_tier_path.is_some() {
                3
            } else {
                2
            }
        );
    }

    /// Record an access event
    ///
    /// This should be called from the SST engine's read path to track access patterns.
    /// Access events are used by the tiering policy engine to make migration decisions.
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection name
    /// * `item_id` - Vector ID or file ID being accessed
    /// * `access_type` - Type of access (Read, Write, Scan)
    /// * `bytes` - Bytes accessed
    pub async fn record_access(
        &self,
        collection: &str,
        item_id: &str,
        access_type: AccessType,
        bytes: u64,
    ) {
        if !self.config.enabled {
            return;
        }

        let event = AccessEvent {
            item_id: item_id.to_string(),
            collection: collection.to_string(),
            timestamp: Instant::now(),
            access_type,
            bytes,
        };

        self.engine.access_tracker().record(event).await;
    }

    /// Determine the target tier for newly flushed data
    ///
    /// This should be called from the flush path to determine where to place new data.
    /// Currently returns the configured default tier.
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection name
    /// * `data_size` - Size of data being flushed in bytes
    ///
    /// # Returns
    ///
    /// The target performance tier for the data
    pub async fn determine_flush_tier(
        &self,
        collection: &str,
        _data_size: u64,
    ) -> PerformanceTier {
        // Phase 6 data plane: operator pins override the default tier.
        // When the collection is pinned to `memory`/`nvme_ssd`/`cloud`,
        // the flush lands on the corresponding `PerformanceTier`. When
        // unpinned, fall back to the access-pattern-policy default.
        if let Some(registry) = &self.pin_registry {
            if let Some(pin) = registry.get(collection) {
                return pin.target.to_performance_tier();
            }
        }
        self.config.default_tier
    }

    /// Get the storage path for a specific tier
    ///
    /// Returns the configured path for the given tier, or None if not configured.
    pub fn get_tier_path(&self, tier: PerformanceTier) -> Option<&str> {
        match tier {
            PerformanceTier::Hot => self.config.hot_tier_path.as_deref(),
            PerformanceTier::Warm => self.config.warm_tier_path.as_deref(),
            PerformanceTier::Cold => self.config.cold_tier_path.as_deref(),
            PerformanceTier::Archive => self.config.archive_tier_path.as_deref(),
        }
    }

    /// Evaluate a collection for potential migrations
    ///
    /// This should be called from the compaction path or as a background task.
    /// Returns a list of migration tasks that should be executed.
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection name to evaluate
    ///
    /// # Returns
    ///
    /// List of migration tasks to execute
    pub async fn evaluate_collection(&self, collection: &str) -> Result<Vec<MigrationTask>> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        let tasks = self.engine.evaluate_collection(collection).await?;

        // Phase 6 data plane: filter out migrations that would move the
        // collection AWAY from its pinned tier. The pin is the
        // operator's explicit intent, so the policy engine's
        // access-pattern-driven proposal is suppressed for that
        // collection. Migrations TOWARDS the pinned tier are kept —
        // that's how a freshly-pinned collection actually catches up.
        let filtered = match self
            .pin_registry
            .as_ref()
            .and_then(|r| r.get(collection))
        {
            Some(pin) => {
                let pinned_tier = pin.target.to_performance_tier();
                let kept: Vec<MigrationTask> = tasks
                    .into_iter()
                    .filter(|task| {
                        // Keep when the migration moves the collection
                        // toward its pinned tier; drop otherwise.
                        task.target_tier == pinned_tier
                    })
                    .collect();
                if !kept.is_empty() {
                    tracing::debug!(
                        collection = %collection,
                        pinned_tier = ?pinned_tier,
                        retained = kept.len(),
                        "tiering: pin-aware filtering kept migrations toward pinned tier"
                    );
                }
                kept
            }
            None => tasks,
        };

        Ok(filtered)
    }

    /// Evaluate all tracked collections for migrations
    ///
    /// This is typically called by the background evaluation loop.
    pub async fn evaluate_all(&self) -> Result<Vec<MigrationTask>> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        self.engine.evaluate_all().await
    }

    /// Record the completion of a migration task
    ///
    /// This should be called after successfully moving data between tiers.
    pub async fn record_migration_complete(&self, result: &MigrationResult) {
        if !self.config.enabled {
            return;
        }

        self.engine.record_migration_complete(result).await;
    }

    /// Get tiering statistics
    pub async fn get_stats(&self) -> TieringStats {
        self.engine.get_stats().await
    }

    /// Get current policies
    pub async fn get_policies(&self) -> Vec<TieringPolicy> {
        self.engine.get_policies().await
    }

    /// Add a custom tiering policy
    pub async fn add_policy(&self, policy: TieringPolicy) {
        self.engine.add_policy(policy).await;
    }

    /// Remove a policy by name
    pub async fn remove_policy(&self, name: &str) -> bool {
        self.engine.remove_policy(name).await
    }

    /// Get the tiering engine for advanced operations
    pub fn tiering_engine(&self) -> &TieringPolicyEngine {
        &self.engine
    }

    /// Build tiering metadata from collection info
    ///
    /// Helper method to create TieringMetadata from collection information.
    /// This is useful when evaluating specific items.
    pub fn build_metadata(
        collection: &str,
        item_id: &str,
        current_tier: PerformanceTier,
        age: Duration,
        access_count: u64,
        size_bytes: u64,
    ) -> TieringMetadata {
        TieringMetadata::new(item_id, collection, current_tier)
            .with_age(age)
            .with_access_count(access_count)
            .with_size(size_bytes)
    }
}

/// Integration status for health checks
#[derive(Debug, Clone)]
pub struct TieringIntegrationStatus {
    /// Whether tiering is enabled
    pub enabled: bool,
    /// Whether the integration is running
    pub running: bool,
    /// Uptime in seconds
    pub uptime_secs: Option<u64>,
    /// Number of active policies
    pub active_policies: usize,
    /// Current statistics
    pub stats: TieringStats,
}

impl SstTieringIntegration {
    /// Get integration status for health checks
    pub async fn get_status(&self) -> TieringIntegrationStatus {
        let running = *self.running.read().await;
        let uptime_secs = self.started_at.map(|t| t.elapsed().as_secs());
        let policies = self.engine.get_policies().await;
        let stats = self.engine.get_stats().await;

        TieringIntegrationStatus {
            enabled: self.config.enabled,
            running,
            uptime_secs,
            active_policies: policies.len(),
            stats,
        }
    }
}

// ============================================================================
// Deferred: Future Integration Points
// ============================================================================
//
// The following integration points are documented for future implementation:
//
// 1. **Flush Path Integration** (src/storage/engines/impls/sst/flush/operations.rs)
//    - After flushing data to disk, call `determine_flush_tier()` to select target path
//    - Write data to the appropriate tier-specific storage location
//    - Record the initial tier in block metadata
//
// 2. **Search Path Integration** (src/storage/engines/impls/sst/search/coordinator.rs)
//    - After each successful search, call `record_access()` with the accessed vector IDs
//    - This enables access-based tiering policies to work correctly
//
// 3. **Compaction Path Integration** (src/storage/engines/impls/sst/compaction.rs)
//    - During compaction, evaluate blocks for tier migration using `evaluate_collection()`
//    - Execute migrations as part of the compaction process (atomic tier changes)
//    - Update block metadata with new tier information
//
// 4. **Migration Execution** (new module needed)
//    - Implement actual data movement between tier storage locations
//    - Handle cross-storage-backend migrations (local -> S3, etc.)
//    - Ensure atomicity and crash recovery for migrations
//
// 5. **Metadata Integration** (src/storage/engines/impls/sst/manifest.rs)
//    - Add tier information to SSTable and block metadata
//    - Track tier history for debugging and auditing
//    - Support tier-aware block selection during search
//
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_integration_creation() {
        let config = SstTieringConfig::default();
        let integration = SstTieringIntegration::new(config).unwrap();
        assert!(!integration.is_enabled());
    }

    #[tokio::test]
    async fn test_integration_enabled() {
        let config = SstTieringConfig {
            enabled: true,
            ..Default::default()
        };
        let integration = SstTieringIntegration::new(config).unwrap();
        assert!(integration.is_enabled());
    }

    #[tokio::test]
    async fn test_record_access_disabled() {
        let config = SstTieringConfig::default();
        let integration = SstTieringIntegration::new(config).unwrap();

        // Should not panic when disabled
        integration
            .record_access("test", "item1", AccessType::Read, 1024)
            .await;
    }

    #[tokio::test]
    async fn test_record_access_enabled() {
        let config = SstTieringConfig {
            enabled: true,
            ..Default::default()
        };
        let integration = SstTieringIntegration::new(config).unwrap();

        integration
            .record_access("test", "item1", AccessType::Read, 1024)
            .await;
        integration
            .record_access("test", "item1", AccessType::Read, 1024)
            .await;

        // Access should be tracked
        let pattern = integration
            .engine
            .access_tracker()
            .get_pattern("test", "item1")
            .await;
        assert!(pattern.is_some());
        assert_eq!(pattern.unwrap().access_count, 2);
    }

    #[tokio::test]
    async fn test_determine_flush_tier() {
        let config = SstTieringConfig {
            enabled: true,
            default_tier: PerformanceTier::Warm,
            ..Default::default()
        };
        let integration = SstTieringIntegration::new(config).unwrap();

        let tier = integration.determine_flush_tier("test", 1024 * 1024).await;
        assert_eq!(tier, PerformanceTier::Warm);
    }

    // ── Phase 6 Slice 6.4: pin-aware data plane ─────────────────────────

    #[tokio::test]
    async fn pin_registry_unconfigured_means_flush_tier_uses_config_default() {
        // Regression: no pin registry attached → behaviour is unchanged
        // (config.default_tier wins). Guards against the integration
        // accidentally picking up a stale default from somewhere else.
        let config = SstTieringConfig {
            enabled: true,
            default_tier: PerformanceTier::Warm,
            ..Default::default()
        };
        let integration = SstTieringIntegration::new(config).unwrap();
        assert!(!integration.pin_registry_configured());

        let tier = integration.determine_flush_tier("test", 1024).await;
        assert_eq!(tier, PerformanceTier::Warm);
    }

    #[tokio::test]
    async fn pin_overrides_flush_tier_to_pinned_target() {
        use crate::storage::collection_pinning::{
            new_shared, CollectionPinTarget,
        };
        let config = SstTieringConfig {
            enabled: true,
            // Default is Warm — pin should override to Hot when the
            // collection is pinned to memory.
            default_tier: PerformanceTier::Warm,
            ..Default::default()
        };
        let registry = new_shared();
        registry.pin("hot-coll", CollectionPinTarget::Memory, 1);
        let integration = SstTieringIntegration::new(config)
            .unwrap()
            .with_pin_registry(registry.clone());
        assert!(integration.pin_registry_configured());

        // Pinned collection → memory → Hot tier.
        let tier = integration.determine_flush_tier("hot-coll", 1024).await;
        assert_eq!(tier, PerformanceTier::Hot);

        // Unpinned collection → fall back to config default.
        let other = integration.determine_flush_tier("other-coll", 1024).await;
        assert_eq!(other, PerformanceTier::Warm);
    }

    #[tokio::test]
    async fn pin_to_cloud_routes_flush_to_cold_tier() {
        use crate::storage::collection_pinning::{
            new_shared, CollectionPinTarget,
        };
        let config = SstTieringConfig {
            enabled: true,
            default_tier: PerformanceTier::Warm,
            ..Default::default()
        };
        let registry = new_shared();
        registry.pin("archive-bound", CollectionPinTarget::Cloud, 1);
        let integration = SstTieringIntegration::new(config)
            .unwrap()
            .with_pin_registry(registry);

        let tier = integration
            .determine_flush_tier("archive-bound", 1024)
            .await;
        assert_eq!(
            tier,
            PerformanceTier::Cold,
            "Cloud pin target maps to Cold performance tier"
        );
    }

    #[tokio::test]
    async fn unpinning_collection_restores_config_default_flush_tier() {
        use crate::storage::collection_pinning::{
            new_shared, CollectionPinTarget,
        };
        let config = SstTieringConfig {
            enabled: true,
            default_tier: PerformanceTier::Warm,
            ..Default::default()
        };
        let registry = new_shared();
        registry.pin("coll", CollectionPinTarget::Memory, 1);
        let integration = SstTieringIntegration::new(config)
            .unwrap()
            .with_pin_registry(registry.clone());

        // While pinned: Hot.
        assert_eq!(
            integration.determine_flush_tier("coll", 1024).await,
            PerformanceTier::Hot
        );

        // After unpin: back to config default.
        registry.unpin("coll");
        assert_eq!(
            integration.determine_flush_tier("coll", 1024).await,
            PerformanceTier::Warm
        );
    }

    #[tokio::test]
    async fn test_start_stop() {
        let config = SstTieringConfig {
            enabled: true,
            auto_evaluate: false, // Disable auto-eval for testing
            ..Default::default()
        };
        let mut integration = SstTieringIntegration::new(config).unwrap();

        integration.start().await.unwrap();

        let status = integration.get_status().await;
        assert!(status.running);
        assert!(status.active_policies >= 2); // Default policies added

        integration.stop().await;

        let status = integration.get_status().await;
        assert!(!status.running);
    }

    #[tokio::test]
    async fn test_tier_paths() {
        let config = SstTieringConfig {
            enabled: true,
            hot_tier_path: Some("file:///data/hot".to_string()),
            warm_tier_path: Some("file:///data/warm".to_string()),
            cold_tier_path: Some("s3://bucket/cold".to_string()),
            ..Default::default()
        };
        let integration = SstTieringIntegration::new(config).unwrap();

        assert_eq!(
            integration.get_tier_path(PerformanceTier::Hot),
            Some("file:///data/hot")
        );
        assert_eq!(
            integration.get_tier_path(PerformanceTier::Warm),
            Some("file:///data/warm")
        );
        assert_eq!(
            integration.get_tier_path(PerformanceTier::Cold),
            Some("s3://bucket/cold")
        );
        assert_eq!(integration.get_tier_path(PerformanceTier::Archive), None);
    }

    #[tokio::test]
    async fn test_build_metadata() {
        let metadata = SstTieringIntegration::build_metadata(
            "test_collection",
            "item1",
            PerformanceTier::Warm,
            Duration::from_secs(86400), // 1 day
            50,
            1024 * 1024,
        );

        assert_eq!(metadata.collection, "test_collection");
        assert_eq!(metadata.id, "item1");
        assert_eq!(metadata.current_tier, PerformanceTier::Warm);
        assert_eq!(metadata.access_count, 50);
        assert_eq!(metadata.size_bytes, 1024 * 1024);
    }

    // ── Tier-migration wiring contract tests ────────────────────────────
    //
    // These tests lock in the attachment + dispatch contract between
    // SstEngine and SstTieringIntegration. End-to-end tests that exercise
    // the full search/flush/compaction paths live with their respective
    // coordinators; here we verify the smaller surface that the engine
    // can attach an integration and that the disabled / enabled toggles
    // gate every public hook the engine calls.

    #[tokio::test]
    async fn engine_starts_with_no_tiering_integration() {
        use crate::storage::engines::sst::SstEngine;
        let engine = SstEngine::new().await.unwrap();
        assert!(
            engine.tiering_integration().is_none(),
            "fresh engine must report no tiering integration"
        );
    }

    #[tokio::test]
    async fn engine_attaches_tiering_integration_via_builder() {
        use crate::storage::engines::sst::SstEngine;
        let config = SstTieringConfig {
            enabled: true,
            ..Default::default()
        };
        let integration = Arc::new(SstTieringIntegration::new(config).unwrap());
        let engine = SstEngine::new()
            .await
            .unwrap()
            .with_tiering_integration(Arc::clone(&integration));
        assert!(
            engine.tiering_integration().is_some(),
            "engine must expose the attached integration"
        );
    }

    #[tokio::test]
    async fn disabled_integration_records_no_access_events() {
        // When `enabled=false`, the search-path hook's `record_access`
        // call short-circuits inside the integration and never reaches
        // the AccessTracker. This is what makes the hook a true no-op
        // for legacy callers who haven't opted into tiering.
        let config = SstTieringConfig::default(); // enabled=false
        let integration = SstTieringIntegration::new(config).unwrap();

        integration
            .record_access("c1", "v1", AccessType::Read, 1024)
            .await;

        let pattern = integration
            .engine
            .access_tracker()
            .get_pattern("c1", "v1")
            .await;
        assert!(
            pattern.is_none(),
            "disabled integration must skip access tracker writes entirely"
        );
    }

    #[tokio::test]
    async fn enabled_integration_aggregates_per_item_reads() {
        // The search-path hook calls `record_access` once per result. A
        // single search returning N hits must be visible as N read events
        // on the corresponding pattern. This locks in the cadence so
        // future heuristics (e.g. "promote after K accesses") have stable
        // input semantics.
        let config = SstTieringConfig {
            enabled: true,
            ..Default::default()
        };
        let integration = SstTieringIntegration::new(config).unwrap();

        for _ in 0..3 {
            integration
                .record_access("c1", "v1", AccessType::Read, 2048)
                .await;
        }

        let pattern = integration
            .engine
            .access_tracker()
            .get_pattern("c1", "v1")
            .await
            .expect("pattern must exist after access events");
        assert_eq!(pattern.access_count, 3, "each call must increment count");
    }

    #[tokio::test]
    async fn evaluate_collection_returns_empty_when_disabled() {
        // The compaction-path hook calls `evaluate_collection`. When the
        // integration is disabled it must return Ok(empty) so the
        // compaction coordinator's logging branch reports zero tasks
        // rather than producing spurious migration logs.
        let config = SstTieringConfig::default(); // enabled=false
        let integration = SstTieringIntegration::new(config).unwrap();

        let tasks = integration.evaluate_collection("c1").await.unwrap();
        assert!(
            tasks.is_empty(),
            "disabled integration must propose no migrations"
        );
    }
}
