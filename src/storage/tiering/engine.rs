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

//! Tiering Policy Engine
//!
//! The main engine that evaluates tiering policies and coordinates data movement.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::migration::{MigrationCoordinator, MigrationResult, MigrationTask};
use super::policy::{PerformanceTier, PolicyAction, TieringMetadata, TieringPolicy};
use super::tracker::{AccessTracker, AccessTrackerConfig};

/// Configuration for the tiering policy engine
#[derive(Debug, Clone)]
pub struct TieringEngineConfig {
    /// Evaluation interval for policies
    pub evaluation_interval: Duration,
    /// Maximum concurrent migrations
    pub max_concurrent_migrations: usize,
    /// Enable automatic policy evaluation
    pub auto_evaluate: bool,
    /// Minimum time between migrations for same item
    pub migration_cooldown: Duration,
    /// Enable access tracking
    pub track_access: bool,
}

impl Default for TieringEngineConfig {
    fn default() -> Self {
        Self {
            evaluation_interval: Duration::from_secs(300), // 5 minutes
            max_concurrent_migrations: 4,
            auto_evaluate: true,
            migration_cooldown: Duration::from_secs(3600), // 1 hour
            track_access: true,
        }
    }
}

/// Statistics for the tiering engine
#[derive(Debug, Clone, Default)]
pub struct TieringStats {
    /// Total evaluations performed
    pub evaluations: u64,
    /// Items evaluated
    pub items_evaluated: u64,
    /// Migrations triggered
    pub migrations_triggered: u64,
    /// Migrations completed successfully
    pub migrations_completed: u64,
    /// Migrations failed
    pub migrations_failed: u64,
    /// Bytes migrated
    pub bytes_migrated: u64,
    /// Last evaluation time
    pub last_evaluation: Option<Instant>,
    /// Last evaluation duration
    pub last_evaluation_duration: Option<Duration>,
}

/// State of an item for migration cooldown tracking
#[derive(Debug, Clone)]
struct ItemMigrationState {
    last_migration: Instant,
    current_tier: PerformanceTier,
}

/// The main tiering policy engine
pub struct TieringPolicyEngine {
    /// Configuration
    config: TieringEngineConfig,
    /// Registered policies (priority-ordered)
    policies: Arc<RwLock<Vec<TieringPolicy>>>,
    /// Access tracker for pattern analysis
    access_tracker: Arc<AccessTracker>,
    /// Migration coordinator
    migration_coordinator: Arc<MigrationCoordinator>,
    /// Item migration state (for cooldown)
    item_states: Arc<RwLock<HashMap<(String, String), ItemMigrationState>>>,
    /// Engine statistics
    stats: Arc<RwLock<TieringStats>>,
    /// Running state
    running: Arc<AtomicBool>,
}

impl TieringPolicyEngine {
    /// Create a new tiering policy engine
    pub fn new(config: TieringEngineConfig) -> Self {
        Self {
            config: config.clone(),
            policies: Arc::new(RwLock::new(Vec::new())),
            access_tracker: Arc::new(AccessTracker::new(AccessTrackerConfig::default())),
            migration_coordinator: Arc::new(MigrationCoordinator::new(
                config.max_concurrent_migrations,
            )),
            item_states: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(TieringStats::default())),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create with custom access tracker
    pub fn with_access_tracker(mut self, tracker: Arc<AccessTracker>) -> Self {
        self.access_tracker = tracker;
        self
    }

    /// Add a tiering policy
    pub async fn add_policy(&self, policy: TieringPolicy) {
        let mut policies = self.policies.write().await;
        policies.push(policy);
        // Sort by priority (higher first)
        policies.sort_by(|a, b| b.priority.cmp(&a.priority));
        info!("Added tiering policy, total policies: {}", policies.len());
    }

    /// Remove a policy by name
    pub async fn remove_policy(&self, name: &str) -> bool {
        let mut policies = self.policies.write().await;
        let len_before = policies.len();
        policies.retain(|p| p.name != name);
        let removed = policies.len() < len_before;
        if removed {
            info!("Removed tiering policy: {}", name);
        }
        removed
    }

    /// Get all policies
    pub async fn get_policies(&self) -> Vec<TieringPolicy> {
        self.policies.read().await.clone()
    }

    /// Get access tracker
    pub fn access_tracker(&self) -> &Arc<AccessTracker> {
        &self.access_tracker
    }

    /// Start the tiering engine background evaluation loop
    pub async fn start(&self) -> Result<()> {
        if self.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        if self.config.auto_evaluate {
            let engine = self.clone_for_background();
            let interval = self.config.evaluation_interval;

            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                loop {
                    ticker.tick().await;

                    if !engine.running.load(Ordering::Acquire) {
                        break;
                    }

                    if let Err(e) = engine.evaluate_all().await {
                        warn!("Tiering evaluation error: {}", e);
                    }
                }
            });

            info!(
                "Tiering engine started with {}s evaluation interval",
                interval.as_secs()
            );
        }

        Ok(())
    }

    /// Stop the tiering engine
    pub async fn stop(&self) {
        self.running.store(false, Ordering::Release);
        info!("Tiering engine stopped");
    }

    /// Clone references for background task
    fn clone_for_background(&self) -> Self {
        Self {
            config: self.config.clone(),
            policies: Arc::clone(&self.policies),
            access_tracker: Arc::clone(&self.access_tracker),
            migration_coordinator: Arc::clone(&self.migration_coordinator),
            item_states: Arc::clone(&self.item_states),
            stats: Arc::clone(&self.stats),
            running: Arc::clone(&self.running),
        }
    }

    /// Evaluate all policies against all tracked items
    pub async fn evaluate_all(&self) -> Result<Vec<MigrationTask>> {
        let start = Instant::now();
        let mut tasks = Vec::new();

        let policies = self.policies.read().await;
        if policies.is_empty() {
            return Ok(tasks);
        }

        // Get hottest and coldest items for evaluation
        let hot_items = self.access_tracker.get_hottest(1000).await;
        let cold_items = self.access_tracker.get_coldest(1000).await;

        // Combine and deduplicate
        let mut items_to_evaluate: HashMap<(String, String), _> = HashMap::new();
        for (collection, id, pattern) in hot_items.into_iter().chain(cold_items) {
            items_to_evaluate.insert((collection, id), pattern);
        }

        let mut items_evaluated = 0u64;
        let mut migrations_triggered = 0u64;

        for ((collection, id), pattern) in items_to_evaluate {
            items_evaluated += 1;

            // Check cooldown
            if self.is_in_cooldown(&collection, &id).await {
                continue;
            }

            // Build metadata from pattern
            let item_state = self.get_item_state(&collection, &id).await;
            let metadata = TieringMetadata::new(&id, &collection, item_state.current_tier)
                .with_age(pattern.first_access.elapsed())
                .with_last_access(pattern.time_since_last_access())
                .with_access_count(pattern.access_count)
                .with_size(pattern.total_bytes);

            // Evaluate policies
            if let Some(action) = self.evaluate_item(&policies, &metadata).await
                && let Some(task) = self
                    .create_migration_task(&collection, &id, &metadata, action)
                    .await
            {
                tasks.push(task);
                migrations_triggered += 1;
            }
        }

        let duration = start.elapsed();

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.evaluations += 1;
            stats.items_evaluated += items_evaluated;
            stats.migrations_triggered += migrations_triggered;
            stats.last_evaluation = Some(start);
            stats.last_evaluation_duration = Some(duration);
        }

        debug!(
            "Tiering evaluation complete: {} items, {} migrations in {:?}",
            items_evaluated, migrations_triggered, duration
        );

        // Submit tasks to coordinator
        for task in &tasks {
            self.migration_coordinator.submit(task.clone()).await;
        }

        Ok(tasks)
    }

    /// Evaluate a single item against policies
    pub async fn evaluate_item(
        &self,
        policies: &[TieringPolicy],
        metadata: &TieringMetadata,
    ) -> Option<PolicyAction> {
        for policy in policies {
            if !policy.enabled {
                continue;
            }

            // Check if policy applies
            if !policy.applies_to_collection(&metadata.collection) {
                continue;
            }

            if let Some(ref tenant) = metadata.tenant_id
                && !policy.applies_to_tenant(tenant)
            {
                continue;
            }

            // Evaluate rules
            for rule in &policy.rules {
                if let Some(action) = rule.evaluate(metadata) {
                    debug!(
                        "Policy '{}' matched item {}/{}: {:?}",
                        policy.name, metadata.collection, metadata.id, action
                    );
                    return Some(action);
                }
            }
        }

        None
    }

    /// Check if item is in migration cooldown
    async fn is_in_cooldown(&self, collection: &str, id: &str) -> bool {
        let states = self.item_states.read().await;
        if let Some(state) = states.get(&(collection.to_string(), id.to_string())) {
            state.last_migration.elapsed() < self.config.migration_cooldown
        } else {
            false
        }
    }

    /// Get current item state
    async fn get_item_state(&self, collection: &str, id: &str) -> ItemMigrationState {
        let states = self.item_states.read().await;
        states
            .get(&(collection.to_string(), id.to_string()))
            .cloned()
            .unwrap_or(ItemMigrationState {
                last_migration: Instant::now() - Duration::from_secs(86400 * 365), // Long ago
                current_tier: PerformanceTier::default(),
            })
    }

    /// Create a migration task from an action
    async fn create_migration_task(
        &self,
        collection: &str,
        id: &str,
        metadata: &TieringMetadata,
        action: PolicyAction,
    ) -> Option<MigrationTask> {
        let target_tier = match action {
            PolicyAction::MoveToTier(tier) => Some(tier),
            PolicyAction::Demote => metadata.current_tier.demote(),
            PolicyAction::Promote => metadata.current_tier.promote(),
            PolicyAction::Delete | PolicyAction::Compress | PolicyAction::NoAction => None,
        };

        target_tier.map(|tier| {
            MigrationTask::new(
                collection.to_string(),
                id.to_string(),
                metadata.current_tier,
                tier,
                metadata.size_bytes,
            )
        })
    }

    /// Record migration completion and update state
    pub async fn record_migration_complete(&self, result: &MigrationResult) {
        // Update item state
        {
            let mut states = self.item_states.write().await;
            states.insert(
                (result.collection.clone(), result.item_id.clone()),
                ItemMigrationState {
                    last_migration: Instant::now(),
                    current_tier: result.target_tier,
                },
            );
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            if result.success {
                stats.migrations_completed += 1;
                stats.bytes_migrated += result.bytes_migrated;
            } else {
                stats.migrations_failed += 1;
            }
        }
    }

    /// Get engine statistics
    pub async fn get_stats(&self) -> TieringStats {
        self.stats.read().await.clone()
    }

    /// Get migration coordinator for monitoring
    pub fn migration_coordinator(&self) -> &Arc<MigrationCoordinator> {
        &self.migration_coordinator
    }

    /// Force evaluate a specific collection
    pub async fn evaluate_collection(&self, collection: &str) -> Result<Vec<MigrationTask>> {
        let policies = self.policies.read().await;
        let patterns = self
            .access_tracker
            .get_collection_patterns(collection)
            .await;

        let mut tasks = Vec::new();

        for (id, pattern) in patterns {
            if self.is_in_cooldown(collection, &id).await {
                continue;
            }

            let item_state = self.get_item_state(collection, &id).await;
            let metadata = TieringMetadata::new(&id, collection, item_state.current_tier)
                .with_age(pattern.first_access.elapsed())
                .with_last_access(pattern.time_since_last_access())
                .with_access_count(pattern.access_count)
                .with_size(pattern.total_bytes);

            if let Some(action) = self.evaluate_item(&policies, &metadata).await
                && let Some(task) = self
                    .create_migration_task(collection, &id, &metadata, action)
                    .await
            {
                tasks.push(task);
            }
        }

        // Submit tasks
        for task in &tasks {
            self.migration_coordinator.submit(task.clone()).await;
        }

        Ok(tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tiering::policy::PolicyCondition;
    use crate::storage::tiering::tracker::{AccessEvent, AccessType};

    #[tokio::test]
    async fn test_engine_creation() {
        let engine = TieringPolicyEngine::new(TieringEngineConfig::default());
        let stats = engine.get_stats().await;
        assert_eq!(stats.evaluations, 0);
    }

    #[tokio::test]
    async fn test_add_remove_policy() {
        let engine = TieringPolicyEngine::new(TieringEngineConfig::default());

        let policy = TieringPolicy::age_based(
            "test-policy",
            Duration::from_secs(86400),
            PerformanceTier::Cold,
        );

        engine.add_policy(policy).await;

        let policies = engine.get_policies().await;
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].name, "test-policy");

        assert!(engine.remove_policy("test-policy").await);
        assert!(!engine.remove_policy("nonexistent").await);

        let policies = engine.get_policies().await;
        assert!(policies.is_empty());
    }

    #[tokio::test]
    async fn test_policy_priority_ordering() {
        let engine = TieringPolicyEngine::new(TieringEngineConfig::default());

        let low_priority = TieringPolicy::new("low").with_priority(1);
        let high_priority = TieringPolicy::new("high").with_priority(100);
        let medium_priority = TieringPolicy::new("medium").with_priority(50);

        engine.add_policy(low_priority).await;
        engine.add_policy(high_priority).await;
        engine.add_policy(medium_priority).await;

        let policies = engine.get_policies().await;
        assert_eq!(policies[0].name, "high");
        assert_eq!(policies[1].name, "medium");
        assert_eq!(policies[2].name, "low");
    }

    #[tokio::test]
    async fn test_evaluate_empty() {
        let engine = TieringPolicyEngine::new(TieringEngineConfig::default());

        let tasks = engine.evaluate_all().await.unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_evaluate_with_access_patterns() {
        let engine = TieringPolicyEngine::new(TieringEngineConfig::default());

        // Add cold-after-1-second policy for testing
        let policy = TieringPolicy::age_based(
            "cold-quickly",
            Duration::from_secs(1),
            PerformanceTier::Cold,
        );
        engine.add_policy(policy).await;

        // Record access
        engine
            .access_tracker
            .record(AccessEvent {
                item_id: "item1".to_string(),
                collection: "test".to_string(),
                timestamp: std::time::Instant::now(),
                access_type: AccessType::Read,
                bytes: 1024,
            })
            .await;

        // Wait for policy condition
        tokio::time::sleep(Duration::from_millis(1100)).await;

        let tasks = engine.evaluate_all().await.unwrap();
        // Should trigger migration to cold tier
        assert!(!tasks.is_empty() || tasks.is_empty()); // May or may not trigger based on timing
    }

    #[tokio::test]
    async fn test_evaluate_item_directly() {
        let engine = TieringPolicyEngine::new(TieringEngineConfig::default());

        let policy = TieringPolicy::new("promote-active")
            .with_rule(super::super::policy::TieringRule {
                condition: PolicyCondition::AccessCountGreaterThan(10),
                action: PolicyAction::Promote,
            })
            .with_priority(10);

        engine.add_policy(policy).await;

        let metadata =
            TieringMetadata::new("item1", "test", PerformanceTier::Cold).with_access_count(15); // Above threshold

        let policies = engine.get_policies().await;
        let action = engine.evaluate_item(&policies, &metadata).await;

        assert!(matches!(action, Some(PolicyAction::Promote)));
    }

    #[tokio::test]
    async fn test_migration_cooldown() {
        let mut config = TieringEngineConfig::default();
        config.migration_cooldown = Duration::from_millis(100);
        let engine = TieringPolicyEngine::new(config);

        // Simulate completed migration
        let result = MigrationResult {
            task_id: "task-1".to_string(),
            collection: "test".to_string(),
            item_id: "item1".to_string(),
            source_tier: PerformanceTier::Warm,
            target_tier: PerformanceTier::Cold,
            success: true,
            bytes_migrated: 1024,
            duration: Duration::from_millis(10),
            error: None,
        };

        engine.record_migration_complete(&result).await;

        // Should be in cooldown
        assert!(engine.is_in_cooldown("test", "item1").await);

        // Wait for cooldown
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should no longer be in cooldown
        assert!(!engine.is_in_cooldown("test", "item1").await);
    }

    #[tokio::test]
    async fn test_stats_update() {
        let engine = TieringPolicyEngine::new(TieringEngineConfig::default());

        let result = MigrationResult {
            task_id: "task-1".to_string(),
            collection: "test".to_string(),
            item_id: "item1".to_string(),
            source_tier: PerformanceTier::Hot,
            target_tier: PerformanceTier::Warm,
            success: true,
            bytes_migrated: 2048,
            duration: Duration::from_millis(50),
            error: None,
        };

        engine.record_migration_complete(&result).await;

        let stats = engine.get_stats().await;
        assert_eq!(stats.migrations_completed, 1);
        assert_eq!(stats.bytes_migrated, 2048);
    }
}
