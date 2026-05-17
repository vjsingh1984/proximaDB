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

//! Tiering Policy Definitions
//!
//! Defines the DSL for tiering policies that control data movement between tiers.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Performance tier for storage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum PerformanceTier {
    /// Memory/NVMe, <1ms latency - highest performance
    Hot,
    /// SSD, <10ms latency - balanced performance/cost
    #[default]
    Warm,
    /// HDD/Cloud, <100ms latency - cost-optimized
    Cold,
    /// Glacier-class, <1s latency - archive storage
    Archive,
}

impl PerformanceTier {
    /// Get the next tier down (for demotion)
    pub fn demote(&self) -> Option<Self> {
        match self {
            Self::Hot => Some(Self::Warm),
            Self::Warm => Some(Self::Cold),
            Self::Cold => Some(Self::Archive),
            Self::Archive => None,
        }
    }

    /// Get the next tier up (for promotion)
    pub fn promote(&self) -> Option<Self> {
        match self {
            Self::Hot => None,
            Self::Warm => Some(Self::Hot),
            Self::Cold => Some(Self::Warm),
            Self::Archive => Some(Self::Cold),
        }
    }

    /// Get relative cost factor (lower = cheaper)
    pub fn cost_factor(&self) -> f64 {
        match self {
            Self::Hot => 10.0,
            Self::Warm => 3.0,
            Self::Cold => 1.0,
            Self::Archive => 0.3,
        }
    }

    /// Get expected latency in milliseconds
    pub fn expected_latency_ms(&self) -> u32 {
        match self {
            Self::Hot => 1,
            Self::Warm => 10,
            Self::Cold => 100,
            Self::Archive => 1000,
        }
    }
}

impl std::fmt::Display for PerformanceTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hot => write!(f, "hot"),
            Self::Warm => write!(f, "warm"),
            Self::Cold => write!(f, "cold"),
            Self::Archive => write!(f, "archive"),
        }
    }
}

/// A tiering policy that defines rules for data movement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieringPolicy {
    /// Unique name for this policy
    pub name: String,
    /// Description of the policy
    pub description: Option<String>,
    /// Priority (higher = evaluated first)
    pub priority: u32,
    /// Whether this policy is enabled
    pub enabled: bool,
    /// Rules in this policy (evaluated in order)
    pub rules: Vec<TieringRule>,
    /// Collections this policy applies to (empty = all)
    pub applies_to: Vec<String>,
    /// Tenants this policy applies to (empty = all)
    pub tenants: Vec<String>,
}

impl TieringPolicy {
    /// Create a new tiering policy
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            priority: 0,
            enabled: true,
            rules: Vec::new(),
            applies_to: Vec::new(),
            tenants: Vec::new(),
        }
    }

    /// Create an age-based policy (demote after N days)
    pub fn age_based(
        name: impl Into<String>,
        max_age: Duration,
        target_tier: PerformanceTier,
    ) -> Self {
        Self::new(name).with_rule(TieringRule {
            condition: PolicyCondition::AgeGreaterThan(max_age),
            action: PolicyAction::MoveToTier(target_tier),
        })
    }

    /// Create an access-based policy (promote if accessed N times)
    pub fn access_based(
        name: impl Into<String>,
        min_access_count: u64,
        target_tier: PerformanceTier,
    ) -> Self {
        Self::new(name).with_rule(TieringRule {
            condition: PolicyCondition::AccessCountGreaterThan(min_access_count),
            action: PolicyAction::MoveToTier(target_tier),
        })
    }

    /// Create a size-based policy (tier down when size exceeds threshold)
    pub fn size_based(
        name: impl Into<String>,
        max_size_bytes: u64,
        target_tier: PerformanceTier,
    ) -> Self {
        Self::new(name).with_rule(TieringRule {
            condition: PolicyCondition::SizeGreaterThan(max_size_bytes),
            action: PolicyAction::MoveToTier(target_tier),
        })
    }

    /// Add a rule to the policy
    pub fn with_rule(mut self, rule: TieringRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Set the description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the priority
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

    /// Check if this policy applies to a collection
    pub fn applies_to_collection(&self, collection: &str) -> bool {
        self.applies_to.is_empty() || self.applies_to.iter().any(|c| c == collection)
    }

    /// Check if this policy applies to a tenant
    pub fn applies_to_tenant(&self, tenant: &str) -> bool {
        self.tenants.is_empty() || self.tenants.iter().any(|t| t == tenant)
    }
}

/// A single rule in a tiering policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieringRule {
    /// Condition that triggers the rule
    pub condition: PolicyCondition,
    /// Action to take when condition is met
    pub action: PolicyAction,
}

impl TieringRule {
    /// Evaluate the rule against a data item
    pub fn evaluate(&self, metadata: &TieringMetadata) -> Option<PolicyAction> {
        if self.condition.matches(metadata) {
            Some(self.action.clone())
        } else {
            None
        }
    }
}

/// Conditions that can trigger a tiering action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PolicyCondition {
    /// Data is older than specified duration
    AgeGreaterThan(Duration),
    /// Data is newer than specified duration
    AgeLessThan(Duration),
    /// Data has been accessed more than N times
    AccessCountGreaterThan(u64),
    /// Data has been accessed fewer than N times
    AccessCountLessThan(u64),
    /// Time since last access exceeds duration
    LastAccessOlderThan(Duration),
    /// Data size exceeds threshold
    SizeGreaterThan(u64),
    /// Data size is below threshold
    SizeLessThan(u64),
    /// Data is in specified tier
    CurrentTierIs(PerformanceTier),
    /// Data is NOT in specified tier
    CurrentTierIsNot(PerformanceTier),
    /// Compound AND condition
    And(Vec<PolicyCondition>),
    /// Compound OR condition
    Or(Vec<PolicyCondition>),
    /// Negation
    Not(Box<PolicyCondition>),
}

impl PolicyCondition {
    /// Check if condition matches the metadata
    pub fn matches(&self, metadata: &TieringMetadata) -> bool {
        match self {
            Self::AgeGreaterThan(dur) => metadata.age > *dur,
            Self::AgeLessThan(dur) => metadata.age < *dur,
            Self::AccessCountGreaterThan(n) => metadata.access_count > *n,
            Self::AccessCountLessThan(n) => metadata.access_count < *n,
            Self::LastAccessOlderThan(dur) => metadata.time_since_last_access > *dur,
            Self::SizeGreaterThan(n) => metadata.size_bytes > *n,
            Self::SizeLessThan(n) => metadata.size_bytes < *n,
            Self::CurrentTierIs(tier) => metadata.current_tier == *tier,
            Self::CurrentTierIsNot(tier) => metadata.current_tier != *tier,
            Self::And(conditions) => conditions.iter().all(|c| c.matches(metadata)),
            Self::Or(conditions) => conditions.iter().any(|c| c.matches(metadata)),
            Self::Not(condition) => !condition.matches(metadata),
        }
    }
}

/// Actions that can be taken by a tiering rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PolicyAction {
    /// Move data to specified tier
    MoveToTier(PerformanceTier),
    /// Demote data one tier down
    Demote,
    /// Promote data one tier up
    Promote,
    /// Delete data (for retention policies)
    Delete,
    /// Compress data in place
    Compress,
    /// Take no action
    NoAction,
}

/// Metadata about a data item for tiering decisions
#[derive(Debug, Clone)]
pub struct TieringMetadata {
    /// Item ID
    pub id: String,
    /// Collection name
    pub collection: String,
    /// Tenant ID
    pub tenant_id: Option<String>,
    /// Current storage tier
    pub current_tier: PerformanceTier,
    /// Age since creation
    pub age: Duration,
    /// Time since last access
    pub time_since_last_access: Duration,
    /// Total access count
    pub access_count: u64,
    /// Size in bytes
    pub size_bytes: u64,
    /// Last modification timestamp (nanoseconds)
    pub last_modified_ns: i64,
    /// Custom tags for policy matching
    pub tags: Vec<String>,
}

impl TieringMetadata {
    /// Create new tiering metadata
    pub fn new(
        id: impl Into<String>,
        collection: impl Into<String>,
        current_tier: PerformanceTier,
    ) -> Self {
        Self {
            id: id.into(),
            collection: collection.into(),
            tenant_id: None,
            current_tier,
            age: Duration::ZERO,
            time_since_last_access: Duration::ZERO,
            access_count: 0,
            size_bytes: 0,
            last_modified_ns: 0,
            tags: Vec::new(),
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

    /// Set access count
    pub fn with_access_count(mut self, count: u64) -> Self {
        self.access_count = count;
        self
    }

    /// Set size
    pub fn with_size(mut self, size: u64) -> Self {
        self.size_bytes = size;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_performance_tier_demotion() {
        assert_eq!(PerformanceTier::Hot.demote(), Some(PerformanceTier::Warm));
        assert_eq!(PerformanceTier::Warm.demote(), Some(PerformanceTier::Cold));
        assert_eq!(
            PerformanceTier::Cold.demote(),
            Some(PerformanceTier::Archive)
        );
        assert_eq!(PerformanceTier::Archive.demote(), None);
    }

    #[test]
    fn test_performance_tier_promotion() {
        assert_eq!(
            PerformanceTier::Archive.promote(),
            Some(PerformanceTier::Cold)
        );
        assert_eq!(PerformanceTier::Cold.promote(), Some(PerformanceTier::Warm));
        assert_eq!(PerformanceTier::Warm.promote(), Some(PerformanceTier::Hot));
        assert_eq!(PerformanceTier::Hot.promote(), None);
    }

    #[test]
    fn test_age_based_policy() {
        let policy = TieringPolicy::age_based(
            "cold-after-7d",
            Duration::from_secs(7 * 24 * 3600),
            PerformanceTier::Cold,
        );

        assert_eq!(policy.name, "cold-after-7d");
        assert_eq!(policy.rules.len(), 1);
    }

    #[test]
    fn test_condition_age_greater_than() {
        let metadata = TieringMetadata::new("item1", "collection1", PerformanceTier::Warm)
            .with_age(Duration::from_secs(8 * 24 * 3600));

        let condition = PolicyCondition::AgeGreaterThan(Duration::from_secs(7 * 24 * 3600));
        assert!(condition.matches(&metadata));

        let condition2 = PolicyCondition::AgeGreaterThan(Duration::from_secs(10 * 24 * 3600));
        assert!(!condition2.matches(&metadata));
    }

    #[test]
    fn test_compound_condition() {
        let metadata = TieringMetadata::new("item1", "collection1", PerformanceTier::Warm)
            .with_age(Duration::from_secs(8 * 24 * 3600))
            .with_access_count(5);

        let condition = PolicyCondition::And(vec![
            PolicyCondition::AgeGreaterThan(Duration::from_secs(7 * 24 * 3600)),
            PolicyCondition::AccessCountLessThan(10),
        ]);
        assert!(condition.matches(&metadata));

        let condition2 = PolicyCondition::Or(vec![
            PolicyCondition::AgeGreaterThan(Duration::from_secs(30 * 24 * 3600)),
            PolicyCondition::AccessCountGreaterThan(100),
        ]);
        assert!(!condition2.matches(&metadata));
    }

    #[test]
    fn test_rule_evaluation() {
        let rule = TieringRule {
            condition: PolicyCondition::LastAccessOlderThan(Duration::from_secs(24 * 3600)),
            action: PolicyAction::Demote,
        };

        let metadata = TieringMetadata::new("item1", "collection1", PerformanceTier::Warm)
            .with_last_access(Duration::from_secs(48 * 3600));

        let action = rule.evaluate(&metadata);
        assert!(matches!(action, Some(PolicyAction::Demote)));
    }

    #[test]
    fn test_policy_applies_to() {
        let policy = TieringPolicy::new("test")
            .for_collections(vec!["col1".to_string(), "col2".to_string()])
            .for_tenants(vec!["tenant-a".to_string()]);

        assert!(policy.applies_to_collection("col1"));
        assert!(policy.applies_to_collection("col2"));
        assert!(!policy.applies_to_collection("col3"));

        assert!(policy.applies_to_tenant("tenant-a"));
        assert!(!policy.applies_to_tenant("tenant-b"));
    }

    #[test]
    fn test_empty_filters_apply_to_all() {
        let policy = TieringPolicy::new("test");

        assert!(policy.applies_to_collection("any-collection"));
        assert!(policy.applies_to_tenant("any-tenant"));
    }
}
