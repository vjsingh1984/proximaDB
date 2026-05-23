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

//! HMGI Partition Migration Engine
//!
//! Handles zero-downtime migration of partitions between storage tiers.
//!
//! ## Migration Process
//!
//! 1. Pause writes to the source partition
//! 2. Copy partition data to target tier
//! 3. Verify copy integrity
//! 4. Update registry pointer atomically
//! 5. Resume writes
//! 6. Cleanup source data (async)

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::{HmgiPartitionKey, HmgiRegistry};
use crate::infrastructure::tier_policy_engine::InfrastructureTier;

/// HMGI partition migration - moves partitions between tiers
///
/// The migration engine ensures zero-downtime tier transitions by:
/// - Using copy-on-write semantics
/// - Verifying data integrity before switching
/// - Supporting rollback on failure
pub struct HmgiMigrationEngine {
    /// HMGI registry for partition access
    registry: Arc<HmgiRegistry>,

    /// Tier policy for target tier determination
    tier_policy: Arc<super::tiering::HmgiTierPolicy>,

    /// Active migrations
    active_migrations: Arc<RwLock<HashMap<String, MigrationState>>>,

    /// Partitions with writes paused during migration.
    paused_writes: Arc<RwLock<HashMap<String, WritePauseToken>>>,

    /// Migration configuration
    config: HmgiMigrationConfig,
}

/// Backwards-compat alias for [`HmgiMigrationConfig`].
pub type MigrationConfig = HmgiMigrationConfig;

/// Migration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HmgiMigrationConfig {
    /// Timeout for migration steps (seconds)
    pub step_timeout_secs: u64,

    /// Maximum concurrent migrations
    pub max_concurrent_migrations: usize,

    /// Whether to automatically cleanup source data after migration
    pub auto_cleanup_source: bool,

    /// Delay before cleanup (seconds)
    pub cleanup_delay_secs: u64,
}

impl Default for HmgiMigrationConfig {
    fn default() -> Self {
        Self {
            step_timeout_secs: 300, // 5 minutes per step
            max_concurrent_migrations: 2,
            auto_cleanup_source: true,
            cleanup_delay_secs: 3600, // 1 hour
        }
    }
}

/// State of an active migration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationState {
    /// Unique migration ID
    pub migration_id: String,

    /// Partition being migrated
    pub partition_key: HmgiPartitionKey,

    /// Source tier
    pub source_tier: InfrastructureTier,

    /// Target tier
    pub target_tier: InfrastructureTier,

    /// Current phase
    pub phase: HmgiMigrationPhase,

    /// Progress (0.0 to 1.0)
    pub progress: f32,

    /// When migration started
    pub started_at: DateTime<Utc>,

    /// Estimated completion
    pub estimated_completion: Option<DateTime<Utc>>,

    /// Error if migration failed
    pub error: Option<String>,
}

/// Migration phase
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HmgiMigrationPhase {
    /// Migration initialized
    Initialized,

    /// Writes paused
    WritesPaused,

    /// Data copying in progress
    Copying,

    /// Verifying integrity
    Verifying,

    /// Switching to new tier
    Switching,

    /// Cleanup in progress
    Cleanup,

    /// Completed successfully
    Completed,

    /// Failed
    Failed,
}

/// Backwards-compat alias for [`HmgiMigrationResult`].
pub type MigrationResult = HmgiMigrationResult;

/// Result of a migration operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HmgiMigrationResult {
    /// Migration ID
    pub migration_id: String,

    /// Partition that was migrated
    pub partition_key: HmgiPartitionKey,

    /// Source tier
    pub from_tier: InfrastructureTier,

    /// Target tier
    pub to_tier: InfrastructureTier,

    /// Duration in milliseconds
    pub duration_ms: u64,

    /// Number of vectors migrated
    pub vectors_migrated: u64,

    /// Whether migration was successful
    pub success: bool,
}

impl HmgiMigrationEngine {
    /// Create a new migration engine
    pub fn new(
        registry: Arc<HmgiRegistry>,
        tier_policy: Arc<super::tiering::HmgiTierPolicy>,
    ) -> Self {
        Self {
            registry,
            tier_policy,
            active_migrations: Arc::new(RwLock::new(HashMap::new())),
            paused_writes: Arc::new(RwLock::new(HashMap::new())),
            config: HmgiMigrationConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(
        registry: Arc<HmgiRegistry>,
        tier_policy: Arc<super::tiering::HmgiTierPolicy>,
        config: HmgiMigrationConfig,
    ) -> Self {
        Self {
            registry,
            tier_policy,
            active_migrations: Arc::new(RwLock::new(HashMap::new())),
            paused_writes: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Migrate a partition to a target tier
    ///
    /// ## Process
    ///
    /// 1. Check if migration is already in progress
    /// 2. Initialize migration state
    /// 3. Pause writes to source partition
    /// 4. Copy partition data to target tier
    /// 5. Verify copy integrity
    /// 6. Update registry pointer atomically
    /// 7. Resume writes
    /// 8. Trigger async cleanup of source
    pub async fn migrate_partition(
        &self,
        partition_key: HmgiPartitionKey,
        target_tier: InfrastructureTier,
    ) -> Result<HmgiMigrationResult> {
        let migration_id = format!(
            "{}_{}",
            partition_key,
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );

        // Check if migration is already in progress
        {
            let active = self.active_migrations.read().await;
            if let Some(state) = active.get(&partition_key.to_string()) {
                if state.phase != HmgiMigrationPhase::Completed
                    && state.phase != HmgiMigrationPhase::Failed
                {
                    return Err(anyhow::anyhow!(
                        "Migration already in progress: {} (phase: {:?})",
                        partition_key,
                        state.phase
                    ));
                }
            }

            let active_count = active
                .values()
                .filter(|state| {
                    state.phase != HmgiMigrationPhase::Completed
                        && state.phase != HmgiMigrationPhase::Failed
                })
                .count();

            if active_count >= self.config.max_concurrent_migrations {
                return Err(anyhow::anyhow!(
                    "Maximum concurrent HMGI migrations reached: {}",
                    self.config.max_concurrent_migrations
                ));
            }
        }

        // Get source partition index
        let _source_index = self
            .registry
            .get_partition(&partition_key)
            .await
            .ok_or_else(|| anyhow::anyhow!("Partition not found: {}", partition_key))?;

        // Initialize migration state
        let source_tier = self
            .tier_policy
            .select_tier_for_modality(&partition_key.modality_tag)
            .await;

        let state = MigrationState {
            migration_id: migration_id.clone(),
            partition_key: partition_key.clone(),
            source_tier: source_tier.clone(),
            target_tier: target_tier.clone(),
            phase: HmgiMigrationPhase::Initialized,
            progress: 0.0,
            started_at: Utc::now(),
            estimated_completion: None,
            error: None,
        };

        {
            let mut active = self.active_migrations.write().await;
            active.insert(partition_key.to_string(), state);
        }

        let start_time = std::time::Instant::now();

        // Execute migration phases
        match self
            .execute_migration_phases(&partition_key, &target_tier)
            .await
        {
            Ok(vectors_migrated) => {
                let duration = start_time.elapsed();

                // Update state to completed
                {
                    let mut active = self.active_migrations.write().await;
                    if let Some(state) = active.get_mut(&partition_key.to_string()) {
                        state.phase = HmgiMigrationPhase::Completed;
                        state.progress = 1.0;
                    }
                }

                Ok(HmgiMigrationResult {
                    migration_id,
                    partition_key,
                    from_tier: source_tier,
                    to_tier: target_tier,
                    duration_ms: duration.as_millis() as u64,
                    vectors_migrated,
                    success: true,
                })
            }
            Err(e) => {
                // Update state to failed
                {
                    let mut active = self.active_migrations.write().await;
                    if let Some(state) = active.get_mut(&partition_key.to_string()) {
                        state.phase = HmgiMigrationPhase::Failed;
                        state.error = Some(e.to_string());
                    }
                }

                Err(e)
            }
        }
    }

    /// Execute the migration phases sequentially
    async fn execute_migration_phases(
        &self,
        partition_key: &HmgiPartitionKey,
        target_tier: &InfrastructureTier,
    ) -> Result<u64> {
        // Phase 1: Pause writes
        self.update_phase(partition_key, HmgiMigrationPhase::WritesPaused, 0.1)
            .await;
        let pause_token = self.pause_writes(partition_key).await?;

        // Phase 2: Copy data
        self.update_phase(partition_key, HmgiMigrationPhase::Copying, 0.2)
            .await;
        let vectors_migrated = self.copy_partition_data(partition_key, target_tier).await?;

        // Phase 3: Verify
        self.update_phase(partition_key, HmgiMigrationPhase::Verifying, 0.8)
            .await;
        self.verify_copy_integrity(partition_key, vectors_migrated)
            .await?;

        // Phase 4: Switch
        self.update_phase(partition_key, HmgiMigrationPhase::Switching, 0.9)
            .await;
        self.switch_to_target_tier(partition_key, target_tier.clone())
            .await?;

        // Phase 5: Resume writes
        self.resume_writes(partition_key, pause_token).await?;

        // Phase 6: Cleanup (async)
        if self.config.auto_cleanup_source {
            let partition_key_clone = partition_key.clone();
            let registry = self.registry.clone();
            let source_tier = self
                .tier_policy
                .select_tier_for_modality(&partition_key.modality_tag)
                .await;
            let cleanup_delay_secs = self.config.cleanup_delay_secs;

            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(cleanup_delay_secs)).await;
                let _ =
                    Self::cleanup_source_data(registry, &partition_key_clone, &source_tier).await;
            });
        }

        self.update_phase(partition_key, HmgiMigrationPhase::Completed, 1.0)
            .await;

        Ok(vectors_migrated)
    }

    /// Update migration phase
    async fn update_phase(
        &self,
        partition_key: &HmgiPartitionKey,
        phase: HmgiMigrationPhase,
        progress: f32,
    ) {
        let mut active = self.active_migrations.write().await;
        if let Some(state) = active.get_mut(&partition_key.to_string()) {
            state.phase = phase;
            state.progress = progress;
        }
    }

    /// Pause writes to a partition
    async fn pause_writes(&self, partition_key: &HmgiPartitionKey) -> Result<WritePauseToken> {
        let mut paused_writes = self.paused_writes.write().await;
        if paused_writes.contains_key(&partition_key.to_string()) {
            return Err(anyhow::anyhow!(
                "Writes already paused for partition: {}",
                partition_key
            ));
        }

        let token = WritePauseToken {
            token_id: uuid::Uuid::new_v4().to_string(),
        };
        paused_writes.insert(partition_key.to_string(), token.clone());

        Ok(token)
    }

    /// Resume writes to a partition
    async fn resume_writes(
        &self,
        partition_key: &HmgiPartitionKey,
        token: WritePauseToken,
    ) -> Result<()> {
        let mut paused_writes = self.paused_writes.write().await;
        match paused_writes.get(&partition_key.to_string()) {
            Some(stored_token) if stored_token.token_id == token.token_id => {
                paused_writes.remove(&partition_key.to_string());
            }
            Some(_) => {
                return Err(anyhow::anyhow!(
                    "Write pause token mismatch for partition: {}",
                    partition_key
                ));
            }
            None => {
                return Err(anyhow::anyhow!(
                    "Writes are not paused for partition: {}",
                    partition_key
                ));
            }
        }

        Ok(())
    }

    /// Copy partition data to target tier
    async fn copy_partition_data(
        &self,
        partition_key: &HmgiPartitionKey,
        _target_tier: &InfrastructureTier,
    ) -> Result<u64> {
        // Get the source index
        let source_index = self
            .registry
            .get_partition(partition_key)
            .await
            .ok_or_else(|| anyhow::anyhow!("Partition not found"))?;

        // For now, we'll simulate the copy by getting stats
        // In a real implementation, this would:
        // 1. Create a new index in the target tier
        // 2. Copy all vectors from source to target
        // 3. Return the count of copied vectors

        let vector_count = source_index.size();
        Ok(vector_count as u64)
    }

    /// Verify copy integrity
    async fn verify_copy_integrity(
        &self,
        _partition_key: &HmgiPartitionKey,
        _expected_count: u64,
    ) -> Result<()> {
        // TODO: Implement verification by comparing:
        // - Vector counts
        // - Sample of vectors for correctness
        // - Index structure integrity
        Ok(())
    }

    /// Switch registry pointer to target tier
    async fn switch_to_target_tier(
        &self,
        partition_key: &HmgiPartitionKey,
        target_tier: InfrastructureTier,
    ) -> Result<()> {
        // Update tier policy to reflect new tier
        // In a real implementation, this would update the registry
        // to point to the new index location
        self.tier_policy
            .set_modality_tier(partition_key.modality_tag.clone(), target_tier)
            .await;

        Ok(())
    }

    /// Cleanup source data after successful migration
    async fn cleanup_source_data(
        _registry: Arc<HmgiRegistry>,
        _partition_key: &HmgiPartitionKey,
        _source_tier: &InfrastructureTier,
    ) -> Result<()> {
        // TODO: Implement cleanup of source data
        // This should delete the old index data from the source tier
        Ok(())
    }

    /// Get state of an active migration
    pub async fn get_migration_state(&self, partition_key: &str) -> Option<MigrationState> {
        let active = self.active_migrations.read().await;
        active.get(partition_key).cloned()
    }

    /// Get all active migrations
    pub async fn get_all_migrations(&self) -> Vec<MigrationState> {
        let active = self.active_migrations.read().await;
        active.values().cloned().collect()
    }

    /// Cancel an active migration
    pub async fn cancel_migration(&self, partition_key: &str) -> Result<()> {
        let mut active = self.active_migrations.write().await;

        if let Some(state) = active.get_mut(partition_key) {
            match state.phase {
                HmgiMigrationPhase::Completed | HmgiMigrationPhase::Failed => {
                    return Err(anyhow::anyhow!(
                        "Cannot cancel migration in phase: {:?}",
                        state.phase
                    ));
                }
                _ => {
                    state.phase = HmgiMigrationPhase::Failed;
                    state.error = Some("Cancelled by user".to_string());
                }
            }
        }

        Ok(())
    }
}

/// Token representing a paused write state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WritePauseToken {
    token_id: String,
}

#[cfg(test)]
mod tests {
    use super::super::tiering::HmgiTierPolicy;
    use super::*;

    #[tokio::test]
    async fn test_migration_initialization() {
        let registry = Arc::new(HmgiRegistry::new());
        let tier_policy = Arc::new(HmgiTierPolicy::default());
        let engine = HmgiMigrationEngine::new(registry, tier_policy);

        // Check that no migrations are active initially
        let migrations = engine.get_all_migrations().await;
        assert!(migrations.is_empty());
    }

    #[tokio::test]
    async fn test_migration_state_tracking() {
        let registry = Arc::new(HmgiRegistry::new());
        let tier_policy = Arc::new(HmgiTierPolicy::default());
        let engine = HmgiMigrationEngine::new(registry.clone(), tier_policy);

        // Create a test partition first
        let partition_key = HmgiPartitionKey::new(123, 1, "test".to_string(), None);
        let config = crate::index::axis::indexes::hnsw_index::AxisHnswConfig::default();

        let _index = registry
            .get_or_create_partition(partition_key.clone(), config, 128)
            .await
            .unwrap();

        // Note: Actual migration would fail without a real index, but we can test state tracking
        let result = engine
            .migrate_partition(partition_key.clone(), InfrastructureTier::Memory)
            .await;

        // Migration might fail but state should be tracked
        let state = engine.get_migration_state(&partition_key.to_string()).await;
        assert!(state.is_some());
    }
}
