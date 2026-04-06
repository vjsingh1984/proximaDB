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

//! Retention Management
//!
//! Manages data lifecycle policies including TTL-based expiration and archival.
//! Supports automatic deletion after retention period and archival to cold storage.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Retention policy for a collection or namespace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Unique name for this policy
    pub name: String,
    /// Description of the policy
    pub description: Option<String>,
    /// Whether this policy is enabled
    pub enabled: bool,
    /// Retention rules
    pub rules: Vec<RetentionRule>,
    /// Collections this policy applies to (empty = all)
    pub applies_to: Vec<String>,
    /// Tenants this policy applies to (empty = all)
    pub tenants: Vec<String>,
    /// Priority (higher = evaluated first)
    pub priority: u32,
}

impl RetentionPolicy {
    /// Create a new retention policy
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            enabled: true,
            rules: Vec::new(),
            applies_to: Vec::new(),
            tenants: Vec::new(),
            priority: 0,
        }
    }

    /// Create a simple TTL-based policy
    pub fn ttl(name: impl Into<String>, ttl: Duration) -> Self {
        Self::new(name).with_rule(RetentionRule::ttl(ttl))
    }

    /// Create an archive-after policy
    pub fn archive_after(
        name: impl Into<String>,
        age: Duration,
        archive_url: impl Into<String>,
    ) -> Self {
        Self::new(name).with_rule(RetentionRule::archive_after(age, archive_url))
    }

    /// Add a rule
    pub fn with_rule(mut self, rule: RetentionRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Set description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set priority
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// Restrict to specific collections
    pub fn for_collections(mut self, collections: Vec<String>) -> Self {
        self.applies_to = collections;
        self
    }

    /// Restrict to specific tenants
    pub fn for_tenants(mut self, tenants: Vec<String>) -> Self {
        self.tenants = tenants;
        self
    }

    /// Check if policy applies to a collection
    pub fn applies_to_collection(&self, collection: &str) -> bool {
        self.applies_to.is_empty() || self.applies_to.iter().any(|c| c == collection)
    }

    /// Check if policy applies to a tenant
    pub fn applies_to_tenant(&self, tenant: &str) -> bool {
        self.tenants.is_empty() || self.tenants.iter().any(|t| t == tenant)
    }
}

/// A single retention rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionRule {
    /// Condition that triggers the rule
    pub condition: RetentionCondition,
    /// Action to take when condition is met
    pub action: RetentionAction,
}

impl RetentionRule {
    /// Create a TTL rule that deletes after specified duration
    pub fn ttl(ttl: Duration) -> Self {
        Self {
            condition: RetentionCondition::AgeGreaterThan(ttl),
            action: RetentionAction::Delete,
        }
    }

    /// Create an archive rule
    pub fn archive_after(age: Duration, archive_url: impl Into<String>) -> Self {
        Self {
            condition: RetentionCondition::AgeGreaterThan(age),
            action: RetentionAction::Archive {
                destination_url: archive_url.into(),
                delete_after_archive: true,
            },
        }
    }

    /// Evaluate the rule against item metadata
    pub fn evaluate(&self, metadata: &RetentionMetadata) -> Option<RetentionAction> {
        if self.condition.matches(metadata) {
            Some(self.action.clone())
        } else {
            None
        }
    }
}

/// Conditions that trigger retention actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RetentionCondition {
    /// Data is older than specified duration
    AgeGreaterThan(Duration),
    /// Data hasn't been accessed for specified duration
    LastAccessOlderThan(Duration),
    /// Data size exceeds threshold
    SizeGreaterThan(u64),
    /// Data is in specified tier
    InTier(String),
    /// Compound AND condition
    And(Vec<RetentionCondition>),
    /// Compound OR condition
    Or(Vec<RetentionCondition>),
    /// Always true (for catch-all rules)
    Always,
}

impl RetentionCondition {
    /// Check if condition matches the metadata
    pub fn matches(&self, metadata: &RetentionMetadata) -> bool {
        match self {
            Self::AgeGreaterThan(dur) => metadata.age > *dur,
            Self::LastAccessOlderThan(dur) => metadata.time_since_last_access > *dur,
            Self::SizeGreaterThan(n) => metadata.size_bytes > *n,
            Self::InTier(tier) => metadata.current_tier.as_deref() == Some(tier.as_str()),
            Self::And(conditions) => conditions.iter().all(|c| c.matches(metadata)),
            Self::Or(conditions) => conditions.iter().any(|c| c.matches(metadata)),
            Self::Always => true,
        }
    }
}

/// Actions that can be taken by retention rules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RetentionAction {
    /// Delete the data permanently
    Delete,
    /// Archive to specified URL (S3, GCS, Azure Blob)
    Archive {
        /// Destination URL (e.g., s3://bucket/prefix/)
        destination_url: String,
        /// Delete local data after successful archive
        delete_after_archive: bool,
    },
    /// Compress data in place
    Compress {
        /// Compression algorithm (e.g., "zstd", "lz4", "snappy")
        algorithm: String,
        /// Compression level
        level: u8,
    },
    /// Move to a different storage location
    Relocate {
        /// Destination URL
        destination_url: String,
    },
    /// Take no action
    NoAction,
}

/// Metadata about an item for retention evaluation
#[derive(Debug, Clone)]
pub struct RetentionMetadata {
    /// Item ID
    pub id: String,
    /// Collection name
    pub collection: String,
    /// Tenant ID
    pub tenant_id: Option<String>,
    /// Age since creation
    pub age: Duration,
    /// Time since last access
    pub time_since_last_access: Duration,
    /// Size in bytes
    pub size_bytes: u64,
    /// Current storage tier
    pub current_tier: Option<String>,
    /// Creation timestamp (nanoseconds)
    pub created_ns: i64,
    /// Last access timestamp (nanoseconds)
    pub last_access_ns: i64,
}

impl RetentionMetadata {
    /// Create new retention metadata
    pub fn new(id: impl Into<String>, collection: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            collection: collection.into(),
            tenant_id: None,
            age: Duration::ZERO,
            time_since_last_access: Duration::ZERO,
            size_bytes: 0,
            current_tier: None,
            created_ns: 0,
            last_access_ns: 0,
        }
    }

    /// Set tenant ID
    pub fn with_tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant.into());
        self
    }

    /// Set age
    pub fn with_age(mut self, age: Duration) -> Self {
        self.age = age;
        self
    }

    /// Set last access time
    pub fn with_last_access(mut self, time_since: Duration) -> Self {
        self.time_since_last_access = time_since;
        self
    }

    /// Set size
    pub fn with_size(mut self, size: u64) -> Self {
        self.size_bytes = size;
        self
    }

    /// Set tier
    pub fn with_tier(mut self, tier: impl Into<String>) -> Self {
        self.current_tier = Some(tier.into());
        self
    }
}

/// Status of a retention operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionOperationStatus {
    /// Operation pending
    Pending,
    /// Operation in progress
    InProgress,
    /// Operation completed successfully
    Completed,
    /// Operation failed
    Failed,
}

/// A retention operation task
#[derive(Debug, Clone)]
pub struct RetentionTask {
    /// Unique task ID
    pub id: String,
    /// Collection
    pub collection: String,
    /// Item ID
    pub item_id: String,
    /// Action to perform
    pub action: RetentionAction,
    /// Status
    pub status: RetentionOperationStatus,
    /// Creation time
    pub created_at: Instant,
    /// Error message if failed
    pub error: Option<String>,
}

/// Result of a retention operation
#[derive(Debug, Clone)]
pub struct RetentionResult {
    /// Task ID
    pub task_id: String,
    /// Collection
    pub collection: String,
    /// Item ID
    pub item_id: String,
    /// Action performed
    pub action: RetentionAction,
    /// Whether operation succeeded
    pub success: bool,
    /// Bytes affected
    pub bytes_affected: u64,
    /// Duration
    pub duration: Duration,
    /// Error message if failed
    pub error: Option<String>,
}

/// Statistics for the retention manager
#[derive(Debug, Clone, Default)]
pub struct RetentionStats {
    /// Total evaluations
    pub evaluations: u64,
    /// Items evaluated
    pub items_evaluated: u64,
    /// Items deleted
    pub items_deleted: u64,
    /// Items archived
    pub items_archived: u64,
    /// Bytes deleted
    pub bytes_deleted: u64,
    /// Bytes archived
    pub bytes_archived: u64,
    /// Failed operations
    pub failed_operations: u64,
    /// Last evaluation time
    pub last_evaluation: Option<Instant>,
}

/// Configuration for the retention manager
#[derive(Debug, Clone)]
pub struct RetentionManagerConfig {
    /// Evaluation interval
    pub evaluation_interval: Duration,
    /// Maximum concurrent operations
    pub max_concurrent_ops: usize,
    /// Enable automatic evaluation
    pub auto_evaluate: bool,
    /// Batch size for evaluation
    pub batch_size: usize,
    /// Retry failed operations
    pub retry_failed: bool,
    /// Maximum retries
    pub max_retries: u32,
}

impl Default for RetentionManagerConfig {
    fn default() -> Self {
        Self {
            evaluation_interval: Duration::from_secs(3600), // 1 hour
            max_concurrent_ops: 4,
            auto_evaluate: true,
            batch_size: 1000,
            retry_failed: true,
            max_retries: 3,
        }
    }
}

/// Manages retention policies and executes retention operations
pub struct RetentionManager {
    /// Configuration
    config: RetentionManagerConfig,
    /// Registered policies
    policies: Arc<RwLock<Vec<RetentionPolicy>>>,
    /// Pending tasks
    pending_tasks: Arc<RwLock<Vec<RetentionTask>>>,
    /// Completed results
    completed_results: Arc<RwLock<Vec<RetentionResult>>>,
    /// Statistics
    stats: Arc<RwLock<RetentionStats>>,
    /// Running state
    running: Arc<RwLock<bool>>,
}

impl RetentionManager {
    /// Create a new retention manager
    pub fn new(config: RetentionManagerConfig) -> Self {
        Self {
            config,
            policies: Arc::new(RwLock::new(Vec::new())),
            pending_tasks: Arc::new(RwLock::new(Vec::new())),
            completed_results: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(RwLock::new(RetentionStats::default())),
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// Add a retention policy
    pub async fn add_policy(&self, policy: RetentionPolicy) {
        let mut policies = self.policies.write().await;
        policies.push(policy);
        policies.sort_by(|a, b| b.priority.cmp(&a.priority));
        info!("Added retention policy, total: {}", policies.len());
    }

    /// Remove a policy by name
    pub async fn remove_policy(&self, name: &str) -> bool {
        let mut policies = self.policies.write().await;
        let len_before = policies.len();
        policies.retain(|p| p.name != name);
        policies.len() < len_before
    }

    /// Get all policies
    pub async fn get_policies(&self) -> Vec<RetentionPolicy> {
        self.policies.read().await.clone()
    }

    /// Start the retention manager background loop
    pub async fn start(&self) {
        let mut running = self.running.write().await;
        if *running {
            return;
        }
        *running = true;
        drop(running);

        if self.config.auto_evaluate {
            let manager = self.clone_for_background();
            let interval = self.config.evaluation_interval;

            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                loop {
                    ticker.tick().await;

                    let running = manager.running.read().await;
                    if !*running {
                        break;
                    }
                    drop(running);

                    // Evaluate would be called here with actual item metadata
                    debug!("Retention evaluation cycle");
                }
            });

            info!(
                "Retention manager started with {}s evaluation interval",
                interval.as_secs()
            );
        }
    }

    /// Stop the retention manager
    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
        info!("Retention manager stopped");
    }

    /// Clone for background task
    fn clone_for_background(&self) -> Self {
        Self {
            config: self.config.clone(),
            policies: Arc::clone(&self.policies),
            pending_tasks: Arc::clone(&self.pending_tasks),
            completed_results: Arc::clone(&self.completed_results),
            stats: Arc::clone(&self.stats),
            running: Arc::clone(&self.running),
        }
    }

    /// Evaluate an item against retention policies
    pub async fn evaluate_item(&self, metadata: &RetentionMetadata) -> Option<RetentionAction> {
        let policies = self.policies.read().await;

        for policy in policies.iter() {
            if !policy.enabled {
                continue;
            }

            if !policy.applies_to_collection(&metadata.collection) {
                continue;
            }

            if let Some(ref tenant) = metadata.tenant_id
                && !policy.applies_to_tenant(tenant)
            {
                continue;
            }

            for rule in &policy.rules {
                if let Some(action) = rule.evaluate(metadata) {
                    debug!(
                        "Retention policy '{}' matched {}/{}: {:?}",
                        policy.name, metadata.collection, metadata.id, action
                    );
                    return Some(action);
                }
            }
        }

        None
    }

    /// Evaluate multiple items
    pub async fn evaluate_batch(
        &self,
        items: Vec<RetentionMetadata>,
    ) -> Vec<(RetentionMetadata, Option<RetentionAction>)> {
        let mut results = Vec::with_capacity(items.len());

        for metadata in items {
            let action = self.evaluate_item(&metadata).await;
            results.push((metadata, action));
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.evaluations += 1;
            stats.items_evaluated += results.len() as u64;
            stats.last_evaluation = Some(Instant::now());
        }

        results
    }

    /// Create a retention task
    pub async fn create_task(
        &self,
        collection: &str,
        item_id: &str,
        action: RetentionAction,
    ) -> String {
        let task_id = format!(
            "retention-{}-{}",
            collection,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );

        let task = RetentionTask {
            id: task_id.clone(),
            collection: collection.to_string(),
            item_id: item_id.to_string(),
            action,
            status: RetentionOperationStatus::Pending,
            created_at: Instant::now(),
            error: None,
        };

        let mut pending = self.pending_tasks.write().await;
        pending.push(task);

        task_id
    }

    /// Record task completion
    pub async fn complete_task(&self, result: RetentionResult) {
        // Remove from pending
        {
            let mut pending = self.pending_tasks.write().await;
            pending.retain(|t| t.id != result.task_id);
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            if result.success {
                match &result.action {
                    RetentionAction::Delete => {
                        stats.items_deleted += 1;
                        stats.bytes_deleted += result.bytes_affected;
                    }
                    RetentionAction::Archive { .. } => {
                        stats.items_archived += 1;
                        stats.bytes_archived += result.bytes_affected;
                    }
                    _ => {}
                }
            } else {
                stats.failed_operations += 1;
            }
        }

        // Store result
        {
            let mut completed = self.completed_results.write().await;
            completed.push(result);
            // Keep only recent results
            while completed.len() > 1000 {
                completed.remove(0);
            }
        }
    }

    /// Get pending tasks
    pub async fn get_pending_tasks(&self) -> Vec<RetentionTask> {
        self.pending_tasks.read().await.clone()
    }

    /// Get completed results
    pub async fn get_completed_results(&self, limit: usize) -> Vec<RetentionResult> {
        let completed = self.completed_results.read().await;
        completed.iter().rev().take(limit).cloned().collect()
    }

    /// Get statistics
    pub async fn get_stats(&self) -> RetentionStats {
        self.stats.read().await.clone()
    }

    /// Get items expiring soon (for dashboard/monitoring)
    pub async fn get_expiring_items<'a>(
        &self,
        items: &'a [RetentionMetadata],
        within: Duration,
    ) -> Vec<&'a RetentionMetadata> {
        let policies = self.policies.read().await;
        let mut expiring = Vec::new();

        for metadata in items {
            for policy in policies.iter() {
                if !policy.enabled || !policy.applies_to_collection(&metadata.collection) {
                    continue;
                }

                for rule in &policy.rules {
                    if let RetentionCondition::AgeGreaterThan(ttl) = &rule.condition {
                        // Calculate time until expiry
                        if let Some(time_remaining) = ttl.checked_sub(metadata.age)
                            && time_remaining <= within
                        {
                            expiring.push(metadata);
                            break;
                        }
                    }
                }
            }
        }

        expiring
    }
}

/// Archive destination configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveConfig {
    /// Destination type (s3, gcs, azure)
    pub destination_type: ArchiveDestinationType,
    /// Bucket or container name
    pub bucket: String,
    /// Prefix for archived files
    pub prefix: String,
    /// Region (for S3/GCS)
    pub region: Option<String>,
    /// Compression for archived files
    pub compression: Option<String>,
    /// Encryption at rest
    pub encrypt: bool,
}

/// Types of archive destinations
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArchiveDestinationType {
    /// Amazon S3
    S3,
    /// Google Cloud Storage
    GCS,
    /// Azure Blob Storage
    AzureBlob,
    /// Local filesystem
    LocalFilesystem,
}

impl ArchiveConfig {
    /// Build URL from config
    pub fn to_url(&self) -> String {
        match self.destination_type {
            ArchiveDestinationType::S3 => {
                format!("s3://{}/{}", self.bucket, self.prefix)
            }
            ArchiveDestinationType::GCS => {
                format!("gs://{}/{}", self.bucket, self.prefix)
            }
            ArchiveDestinationType::AzureBlob => {
                format!("azure://{}/{}", self.bucket, self.prefix)
            }
            ArchiveDestinationType::LocalFilesystem => {
                format!("file://{}/{}", self.bucket, self.prefix)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retention_policy_creation() {
        let policy = RetentionPolicy::ttl("7-day-ttl", Duration::from_secs(7 * 24 * 3600));
        assert_eq!(policy.name, "7-day-ttl");
        assert!(policy.enabled);
        assert_eq!(policy.rules.len(), 1);
    }

    #[test]
    fn test_retention_condition_age() {
        let metadata = RetentionMetadata::new("item1", "collection1")
            .with_age(Duration::from_secs(10 * 24 * 3600)); // 10 days

        let condition_7d = RetentionCondition::AgeGreaterThan(Duration::from_secs(7 * 24 * 3600));
        let condition_30d = RetentionCondition::AgeGreaterThan(Duration::from_secs(30 * 24 * 3600));

        assert!(condition_7d.matches(&metadata)); // 10 > 7
        assert!(!condition_30d.matches(&metadata)); // 10 < 30
    }

    #[test]
    fn test_retention_condition_compound() {
        let metadata = RetentionMetadata::new("item1", "collection1")
            .with_age(Duration::from_secs(10 * 24 * 3600))
            .with_size(5000);

        // Old AND large
        let condition = RetentionCondition::And(vec![
            RetentionCondition::AgeGreaterThan(Duration::from_secs(7 * 24 * 3600)),
            RetentionCondition::SizeGreaterThan(1000),
        ]);
        assert!(condition.matches(&metadata));

        // Old OR large
        let condition_or = RetentionCondition::Or(vec![
            RetentionCondition::AgeGreaterThan(Duration::from_secs(30 * 24 * 3600)),
            RetentionCondition::SizeGreaterThan(1000),
        ]);
        assert!(condition_or.matches(&metadata)); // Size matches
    }

    #[test]
    fn test_retention_rule_evaluation() {
        let rule = RetentionRule::ttl(Duration::from_secs(7 * 24 * 3600));

        let old_metadata =
            RetentionMetadata::new("item1", "col1").with_age(Duration::from_secs(10 * 24 * 3600));

        let new_metadata =
            RetentionMetadata::new("item2", "col1").with_age(Duration::from_secs(1 * 24 * 3600));

        assert!(matches!(
            rule.evaluate(&old_metadata),
            Some(RetentionAction::Delete)
        ));
        assert!(rule.evaluate(&new_metadata).is_none());
    }

    #[test]
    fn test_archive_rule() {
        let rule = RetentionRule::archive_after(
            Duration::from_secs(30 * 24 * 3600),
            "s3://archive-bucket/prefix/",
        );

        let old_metadata =
            RetentionMetadata::new("item1", "col1").with_age(Duration::from_secs(60 * 24 * 3600));

        let action = rule.evaluate(&old_metadata);
        assert!(matches!(action, Some(RetentionAction::Archive { .. })));
    }

    #[test]
    fn test_policy_applies_to() {
        let policy = RetentionPolicy::new("test")
            .for_collections(vec!["logs".to_string(), "metrics".to_string()])
            .for_tenants(vec!["tenant-a".to_string()]);

        assert!(policy.applies_to_collection("logs"));
        assert!(policy.applies_to_collection("metrics"));
        assert!(!policy.applies_to_collection("traces"));

        assert!(policy.applies_to_tenant("tenant-a"));
        assert!(!policy.applies_to_tenant("tenant-b"));
    }

    #[test]
    fn test_empty_filters_apply_to_all() {
        let policy = RetentionPolicy::new("global");

        assert!(policy.applies_to_collection("any-collection"));
        assert!(policy.applies_to_tenant("any-tenant"));
    }

    #[tokio::test]
    async fn test_retention_manager_creation() {
        let manager = RetentionManager::new(RetentionManagerConfig::default());
        let stats = manager.get_stats().await;
        assert_eq!(stats.evaluations, 0);
    }

    #[tokio::test]
    async fn test_retention_manager_add_policy() {
        let manager = RetentionManager::new(RetentionManagerConfig::default());

        let policy = RetentionPolicy::ttl("test-ttl", Duration::from_secs(86400));
        manager.add_policy(policy).await;

        let policies = manager.get_policies().await;
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].name, "test-ttl");
    }

    #[tokio::test]
    async fn test_retention_manager_evaluate_item() {
        let manager = RetentionManager::new(RetentionManagerConfig::default());

        let policy = RetentionPolicy::ttl("7-day-ttl", Duration::from_secs(7 * 24 * 3600));
        manager.add_policy(policy).await;

        // Old item - should be deleted
        let old_metadata = RetentionMetadata::new("item1", "collection1")
            .with_age(Duration::from_secs(10 * 24 * 3600));

        let action = manager.evaluate_item(&old_metadata).await;
        assert!(matches!(action, Some(RetentionAction::Delete)));

        // New item - should not be deleted
        let new_metadata = RetentionMetadata::new("item2", "collection1")
            .with_age(Duration::from_secs(1 * 24 * 3600));

        let action = manager.evaluate_item(&new_metadata).await;
        assert!(action.is_none());
    }

    #[tokio::test]
    async fn test_retention_manager_evaluate_batch() {
        let manager = RetentionManager::new(RetentionManagerConfig::default());

        let policy = RetentionPolicy::ttl("test-ttl", Duration::from_secs(7 * 24 * 3600));
        manager.add_policy(policy).await;

        let items = vec![
            RetentionMetadata::new("item1", "col1").with_age(Duration::from_secs(10 * 24 * 3600)),
            RetentionMetadata::new("item2", "col1").with_age(Duration::from_secs(1 * 24 * 3600)),
            RetentionMetadata::new("item3", "col1").with_age(Duration::from_secs(15 * 24 * 3600)),
        ];

        let results = manager.evaluate_batch(items).await;

        assert_eq!(results.len(), 3);
        assert!(results[0].1.is_some()); // Old - delete
        assert!(results[1].1.is_none()); // New - keep
        assert!(results[2].1.is_some()); // Old - delete

        let stats = manager.get_stats().await;
        assert_eq!(stats.evaluations, 1);
        assert_eq!(stats.items_evaluated, 3);
    }

    #[tokio::test]
    async fn test_retention_task_lifecycle() {
        let manager = RetentionManager::new(RetentionManagerConfig::default());

        let task_id = manager
            .create_task("test-col", "item1", RetentionAction::Delete)
            .await;

        let pending = manager.get_pending_tasks().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, task_id);

        // Complete the task
        let result = RetentionResult {
            task_id: task_id.clone(),
            collection: "test-col".to_string(),
            item_id: "item1".to_string(),
            action: RetentionAction::Delete,
            success: true,
            bytes_affected: 1024,
            duration: Duration::from_millis(10),
            error: None,
        };

        manager.complete_task(result).await;

        let pending = manager.get_pending_tasks().await;
        assert!(pending.is_empty());

        let stats = manager.get_stats().await;
        assert_eq!(stats.items_deleted, 1);
        assert_eq!(stats.bytes_deleted, 1024);
    }

    #[test]
    fn test_archive_config_to_url() {
        let s3_config = ArchiveConfig {
            destination_type: ArchiveDestinationType::S3,
            bucket: "my-bucket".to_string(),
            prefix: "archive/2024".to_string(),
            region: Some("us-west-2".to_string()),
            compression: Some("zstd".to_string()),
            encrypt: true,
        };

        assert_eq!(s3_config.to_url(), "s3://my-bucket/archive/2024");

        let gcs_config = ArchiveConfig {
            destination_type: ArchiveDestinationType::GCS,
            bucket: "gcs-bucket".to_string(),
            prefix: "cold-data".to_string(),
            region: None,
            compression: None,
            encrypt: false,
        };

        assert_eq!(gcs_config.to_url(), "gs://gcs-bucket/cold-data");
    }

    #[tokio::test]
    async fn test_get_expiring_items() {
        let manager = RetentionManager::new(RetentionManagerConfig::default());

        // 7-day TTL policy
        let policy = RetentionPolicy::ttl("7-day-ttl", Duration::from_secs(7 * 24 * 3600));
        manager.add_policy(policy).await;

        let items = vec![
            // 6 days old - expires in 1 day
            RetentionMetadata::new("item1", "col1").with_age(Duration::from_secs(6 * 24 * 3600)),
            // 1 day old - expires in 6 days
            RetentionMetadata::new("item2", "col1").with_age(Duration::from_secs(1 * 24 * 3600)),
            // 8 days old - already expired
            RetentionMetadata::new("item3", "col1").with_age(Duration::from_secs(8 * 24 * 3600)),
        ];

        // Find items expiring within 2 days
        let expiring = manager
            .get_expiring_items(&items, Duration::from_secs(2 * 24 * 3600))
            .await;

        // Only item1 should be "expiring soon" (within 2 days)
        assert_eq!(expiring.len(), 1);
        assert_eq!(expiring[0].id, "item1");
    }
}
