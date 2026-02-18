/*
 * Copyright 2025 Vijaykumar Singh
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

//! Event routing for outbound CDC
//!
//! Routes change events to appropriate sinks based on configurable rules.

use std::collections::HashMap;
use std::sync::RwLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::cdc::event::{ChangeEvent, Operation};

/// Rule for routing events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRule {
    /// Rule name
    pub name: String,
    /// Collection patterns to match
    #[serde(default)]
    pub collection_patterns: Vec<String>,
    /// Operation types to match
    #[serde(default)]
    pub operations: Vec<Operation>,
    /// Metadata conditions
    #[serde(default)]
    pub metadata_conditions: Vec<MetadataCondition>,
    /// Target sink IDs
    pub sink_ids: Vec<String>,
    /// Rule priority (lower = higher priority)
    #[serde(default)]
    pub priority: i32,
    /// Whether rule is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Stop processing after this rule matches
    #[serde(default)]
    pub terminal: bool,
}

fn default_true() -> bool {
    true
}

impl RouteRule {
    /// Create a new route rule
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            collection_patterns: Vec::new(),
            operations: Vec::new(),
            metadata_conditions: Vec::new(),
            sink_ids: Vec::new(),
            priority: 0,
            enabled: true,
            terminal: false,
        }
    }

    /// Add a collection pattern
    pub fn with_collection(mut self, pattern: impl Into<String>) -> Self {
        self.collection_patterns.push(pattern.into());
        self
    }

    /// Add operations to match
    pub fn with_operations(mut self, ops: Vec<Operation>) -> Self {
        self.operations = ops;
        self
    }

    /// Add a sink ID
    pub fn with_sink(mut self, sink_id: impl Into<String>) -> Self {
        self.sink_ids.push(sink_id.into());
        self
    }

    /// Set priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Make this a terminal rule
    pub fn terminal(mut self) -> Self {
        self.terminal = true;
        self
    }

    /// Check if rule matches an event
    pub fn matches(&self, event: &ChangeEvent) -> bool {
        if !self.enabled {
            return false;
        }

        // Check collection pattern
        if !self.collection_patterns.is_empty() {
            let collection_match = self
                .collection_patterns
                .iter()
                .any(|p| pattern_matches(p, &event.collection));
            if !collection_match {
                return false;
            }
        }

        // Check operation
        if !self.operations.is_empty() && !self.operations.contains(&event.operation) {
            return false;
        }

        // Check metadata conditions
        if !self.metadata_conditions.is_empty() {
            let metadata_match = self
                .metadata_conditions
                .iter()
                .all(|cond| cond.evaluate(event));
            if !metadata_match {
                return false;
            }
        }

        true
    }
}

/// Condition for metadata-based routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataCondition {
    /// Field name (supports dot notation for nested fields)
    pub field: String,
    /// Comparison operator
    pub operator: ConditionOperator,
    /// Value to compare against
    pub value: serde_json::Value,
}

/// Comparison operators for conditions
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOperator {
    /// Equals
    Eq,
    /// Not equals
    Ne,
    /// Greater than
    Gt,
    /// Greater than or equal
    Gte,
    /// Less than
    Lt,
    /// Less than or equal
    Lte,
    /// Contains (for strings/arrays)
    Contains,
    /// Starts with (for strings)
    StartsWith,
    /// Ends with (for strings)
    EndsWith,
    /// Matches regex
    Regex,
    /// Field exists
    Exists,
    /// Field is null
    IsNull,
    /// In list
    In,
}

impl MetadataCondition {
    /// Create a new condition
    pub fn new(
        field: impl Into<String>,
        operator: ConditionOperator,
        value: serde_json::Value,
    ) -> Self {
        Self {
            field: field.into(),
            operator,
            value,
        }
    }

    /// Evaluate condition against an event
    pub fn evaluate(&self, event: &ChangeEvent) -> bool {
        // Get field value from metadata
        let field_value = self.get_field_value(event);

        match self.operator {
            ConditionOperator::Exists => field_value.is_some(),
            ConditionOperator::IsNull => field_value.as_ref().map(|v| v.is_null()).unwrap_or(true),
            _ => {
                let Some(ref actual) = field_value else {
                    return false;
                };
                self.compare(actual)
            }
        }
    }

    /// Get field value from event metadata
    fn get_field_value(&self, event: &ChangeEvent) -> Option<serde_json::Value> {
        // Handle special fields
        match self.field.as_str() {
            "_collection" => {
                return Some(serde_json::Value::String(event.collection.clone()));
            }
            "_operation" => {
                return Some(serde_json::Value::String(event.operation.to_string()));
            }
            "_lsn" => {
                return Some(serde_json::Value::Number(event.lsn.into()));
            }
            _ => {}
        }

        // Get metadata from after state
        let after_state = event.after.as_ref()?;

        // Navigate through metadata using dot notation
        let parts: Vec<&str> = self.field.split('.').collect();
        let first_part = parts.first()?;
        let mut current = after_state.metadata.get(*first_part)?.clone();

        for part in parts.iter().skip(1) {
            match current {
                serde_json::Value::Object(ref map) => {
                    current = map.get(*part)?.clone();
                }
                serde_json::Value::Array(ref arr) => {
                    let idx: usize = part.parse().ok()?;
                    current = arr.get(idx)?.clone();
                }
                _ => return None,
            }
        }

        Some(current)
    }

    /// Compare values based on operator
    fn compare(&self, actual: &serde_json::Value) -> bool {
        match self.operator {
            ConditionOperator::Eq => actual == &self.value,
            ConditionOperator::Ne => actual != &self.value,
            ConditionOperator::Gt => self.compare_numeric(actual, |a, b| a > b),
            ConditionOperator::Gte => self.compare_numeric(actual, |a, b| a >= b),
            ConditionOperator::Lt => self.compare_numeric(actual, |a, b| a < b),
            ConditionOperator::Lte => self.compare_numeric(actual, |a, b| a <= b),
            ConditionOperator::Contains => self.check_contains(actual),
            ConditionOperator::StartsWith => self.check_starts_with(actual),
            ConditionOperator::EndsWith => self.check_ends_with(actual),
            ConditionOperator::Regex => self.check_regex(actual),
            ConditionOperator::In => self.check_in(actual),
            ConditionOperator::Exists | ConditionOperator::IsNull => {
                // Already handled above
                true
            }
        }
    }

    fn compare_numeric(&self, actual: &serde_json::Value, op: fn(f64, f64) -> bool) -> bool {
        let a = actual.as_f64();
        let b = self.value.as_f64();
        match (a, b) {
            (Some(a), Some(b)) => op(a, b),
            _ => false,
        }
    }

    fn check_contains(&self, actual: &serde_json::Value) -> bool {
        match (actual, &self.value) {
            (serde_json::Value::String(s), serde_json::Value::String(needle)) => {
                s.contains(needle.as_str())
            }
            (serde_json::Value::Array(arr), needle) => arr.contains(needle),
            _ => false,
        }
    }

    fn check_starts_with(&self, actual: &serde_json::Value) -> bool {
        match (actual, &self.value) {
            (serde_json::Value::String(s), serde_json::Value::String(prefix)) => {
                s.starts_with(prefix.as_str())
            }
            _ => false,
        }
    }

    fn check_ends_with(&self, actual: &serde_json::Value) -> bool {
        match (actual, &self.value) {
            (serde_json::Value::String(s), serde_json::Value::String(suffix)) => {
                s.ends_with(suffix.as_str())
            }
            _ => false,
        }
    }

    fn check_regex(&self, actual: &serde_json::Value) -> bool {
        match (actual, &self.value) {
            (serde_json::Value::String(s), serde_json::Value::String(pattern)) => {
                Regex::new(pattern)
                    .map(|re| re.is_match(s))
                    .unwrap_or(false)
            }
            _ => false,
        }
    }

    fn check_in(&self, actual: &serde_json::Value) -> bool {
        match &self.value {
            serde_json::Value::Array(arr) => arr.contains(actual),
            _ => false,
        }
    }
}

/// Result of routing decision
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    /// Event being routed
    pub event: ChangeEvent,
    /// Matched rules
    pub matched_rules: Vec<String>,
    /// Target sink IDs
    pub sink_ids: Vec<String>,
    /// Whether any rule matched
    pub has_match: bool,
}

impl RoutingDecision {
    /// Create decision for no matches
    pub fn no_match(event: ChangeEvent) -> Self {
        Self {
            event,
            matched_rules: Vec::new(),
            sink_ids: Vec::new(),
            has_match: false,
        }
    }

    /// Create decision with matches
    pub fn with_matches(event: ChangeEvent, rules: Vec<String>, sinks: Vec<String>) -> Self {
        Self {
            event,
            matched_rules: rules,
            sink_ids: sinks,
            has_match: true,
        }
    }
}

/// Router for CDC events
pub struct EventRouter {
    /// Routing rules (sorted by priority)
    rules: RwLock<Vec<RouteRule>>,
    /// Default sink IDs (when no rules match)
    default_sinks: RwLock<Vec<String>>,
    /// Statistics
    stats: RwLock<RouterStats>,
}

/// Statistics for routing
#[derive(Debug, Clone, Default)]
pub struct RouterStats {
    /// Total events routed
    pub events_routed: u64,
    /// Events matched by rules
    pub events_matched: u64,
    /// Events sent to default sinks
    pub events_default: u64,
    /// Events dropped (no match, no default)
    pub events_dropped: u64,
    /// Per-rule match counts
    pub rule_matches: HashMap<String, u64>,
    /// Per-sink event counts
    pub sink_events: HashMap<String, u64>,
}

impl Default for EventRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl EventRouter {
    /// Create a new event router
    pub fn new() -> Self {
        Self {
            rules: RwLock::new(Vec::new()),
            default_sinks: RwLock::new(Vec::new()),
            stats: RwLock::new(RouterStats::default()),
        }
    }

    /// Add a routing rule
    pub fn add_rule(&self, rule: RouteRule) {
        let mut rules = self.rules.write().unwrap();
        rules.push(rule);
        rules.sort_by_key(|r| r.priority);
    }

    /// Remove a rule by name
    pub fn remove_rule(&self, name: &str) -> bool {
        let mut rules = self.rules.write().unwrap();
        let len_before = rules.len();
        rules.retain(|r| r.name != name);
        rules.len() < len_before
    }

    /// Set default sinks
    pub fn set_default_sinks(&self, sinks: Vec<String>) {
        *self.default_sinks.write().unwrap() = sinks;
    }

    /// Route an event
    pub fn route(&self, event: ChangeEvent) -> RoutingDecision {
        let rules = self.rules.read().unwrap();
        let mut matched_rules = Vec::new();
        let mut sink_ids = Vec::new();

        let mut stats = self.stats.write().unwrap();
        stats.events_routed += 1;

        for rule in rules.iter() {
            if rule.matches(&event) {
                matched_rules.push(rule.name.clone());

                // Update rule stats
                *stats.rule_matches.entry(rule.name.clone()).or_insert(0) += 1;

                // Add sink IDs
                for sink_id in &rule.sink_ids {
                    if !sink_ids.contains(sink_id) {
                        sink_ids.push(sink_id.clone());
                        *stats.sink_events.entry(sink_id.clone()).or_insert(0) += 1;
                    }
                }

                // Stop if terminal rule
                if rule.terminal {
                    break;
                }
            }
        }
        drop(rules);

        if !matched_rules.is_empty() {
            stats.events_matched += 1;
            return RoutingDecision::with_matches(event, matched_rules, sink_ids);
        }

        // Use default sinks if no rules matched
        let default_sinks = self.default_sinks.read().unwrap();
        if !default_sinks.is_empty() {
            stats.events_default += 1;
            for sink_id in default_sinks.iter() {
                *stats.sink_events.entry(sink_id.clone()).or_insert(0) += 1;
            }
            return RoutingDecision::with_matches(
                event,
                vec!["_default".to_string()],
                default_sinks.clone(),
            );
        }

        stats.events_dropped += 1;
        RoutingDecision::no_match(event)
    }

    /// Route multiple events
    pub fn route_batch(&self, events: Vec<ChangeEvent>) -> Vec<RoutingDecision> {
        events.into_iter().map(|e| self.route(e)).collect()
    }

    /// Get current rules
    pub fn rules(&self) -> Vec<RouteRule> {
        self.rules.read().unwrap().clone()
    }

    /// Get statistics
    pub fn stats(&self) -> RouterStats {
        self.stats.read().unwrap().clone()
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        *self.stats.write().unwrap() = RouterStats::default();
    }

    /// Enable/disable a rule
    pub fn set_rule_enabled(&self, name: &str, enabled: bool) -> bool {
        let mut rules = self.rules.write().unwrap();
        for rule in rules.iter_mut() {
            if rule.name == name {
                rule.enabled = enabled;
                return true;
            }
        }
        false
    }
}

/// Check if a pattern matches a string
fn pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if pattern.contains('*') {
        // Simple glob-like matching
        let parts: Vec<&str> = pattern.split('*').collect();

        if parts.len() == 2 {
            // Single wildcard
            if pattern.starts_with('*') {
                return value.ends_with(parts[1]);
            } else if pattern.ends_with('*') {
                return value.starts_with(parts[0]);
            } else {
                return value.starts_with(parts[0]) && value.ends_with(parts[1]);
            }
        }

        // Try regex for complex patterns
        let regex_pattern = pattern.replace('.', "\\.").replace('*', ".*");
        if let Ok(re) = Regex::new(&format!("^{}$", regex_pattern)) {
            return re.is_match(value);
        }
    }

    pattern == value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::event::{RecordState, SourceInfo};
    use std::collections::HashMap;

    fn create_test_event(collection: &str, op: Operation) -> ChangeEvent {
        let mut event = ChangeEvent::new(
            SourceInfo::proximadb("testdb", "test_server"),
            op,
            collection,
            "key_1",
        );
        // Set metadata in the after state
        let mut metadata = HashMap::new();
        metadata.insert("tenant".to_string(), serde_json::json!("acme"));
        metadata.insert("priority".to_string(), serde_json::json!(5));
        metadata.insert("tags".to_string(), serde_json::json!(["important", "sync"]));
        event.after = Some(RecordState::with_metadata(metadata));
        event
    }

    #[test]
    fn test_pattern_matching() {
        assert!(pattern_matches("*", "anything"));
        assert!(pattern_matches("products", "products"));
        assert!(!pattern_matches("products", "users"));

        assert!(pattern_matches("products*", "products"));
        assert!(pattern_matches("products*", "products_v2"));
        assert!(!pattern_matches("products*", "my_products"));

        assert!(pattern_matches("*_events", "user_events"));
        assert!(pattern_matches("*_events", "order_events"));
        assert!(!pattern_matches("*_events", "events"));
    }

    #[test]
    fn test_route_rule_matching() {
        let rule = RouteRule::new("test_rule")
            .with_collection("products*")
            .with_operations(vec![Operation::Insert, Operation::Update])
            .with_sink("kafka");

        let event1 = create_test_event("products", Operation::Insert);
        assert!(rule.matches(&event1));

        let event2 = create_test_event("products_v2", Operation::Update);
        assert!(rule.matches(&event2));

        let event3 = create_test_event("users", Operation::Insert);
        assert!(!rule.matches(&event3));

        let event4 = create_test_event("products", Operation::Delete);
        assert!(!rule.matches(&event4));
    }

    #[test]
    fn test_metadata_condition() {
        let event = create_test_event("products", Operation::Insert);

        // Equals
        let cond =
            MetadataCondition::new("tenant", ConditionOperator::Eq, serde_json::json!("acme"));
        assert!(cond.evaluate(&event));

        // Greater than
        let cond = MetadataCondition::new("priority", ConditionOperator::Gt, serde_json::json!(3));
        assert!(cond.evaluate(&event));

        // Contains (array)
        let cond = MetadataCondition::new(
            "tags",
            ConditionOperator::Contains,
            serde_json::json!("important"),
        );
        assert!(cond.evaluate(&event));

        // Exists
        let cond =
            MetadataCondition::new("tenant", ConditionOperator::Exists, serde_json::Value::Null);
        assert!(cond.evaluate(&event));

        // Not exists
        let cond = MetadataCondition::new(
            "missing",
            ConditionOperator::Exists,
            serde_json::Value::Null,
        );
        assert!(!cond.evaluate(&event));
    }

    #[test]
    fn test_router_basic() {
        let router = EventRouter::new();

        router.add_rule(
            RouteRule::new("products_to_kafka")
                .with_collection("products*")
                .with_sink("kafka"),
        );

        router.add_rule(
            RouteRule::new("users_to_webhook")
                .with_collection("users")
                .with_sink("webhook"),
        );

        // Products should go to Kafka
        let event = create_test_event("products", Operation::Insert);
        let decision = router.route(event);
        assert!(decision.has_match);
        assert!(decision.sink_ids.contains(&"kafka".to_string()));

        // Users should go to webhook
        let event = create_test_event("users", Operation::Insert);
        let decision = router.route(event);
        assert!(decision.has_match);
        assert!(decision.sink_ids.contains(&"webhook".to_string()));
    }

    #[test]
    fn test_router_multiple_matches() {
        let router = EventRouter::new();

        router.add_rule(
            RouteRule::new("all_to_kafka")
                .with_collection("*")
                .with_sink("kafka"),
        );

        router.add_rule(
            RouteRule::new("products_to_webhook")
                .with_collection("products")
                .with_sink("webhook"),
        );

        // Products should match both rules
        let event = create_test_event("products", Operation::Insert);
        let decision = router.route(event);
        assert!(decision.has_match);
        assert!(decision.sink_ids.contains(&"kafka".to_string()));
        assert!(decision.sink_ids.contains(&"webhook".to_string()));
        assert_eq!(decision.matched_rules.len(), 2);
    }

    #[test]
    fn test_router_terminal_rule() {
        let router = EventRouter::new();

        router.add_rule(
            RouteRule::new("products_only")
                .with_collection("products")
                .with_sink("kafka")
                .with_priority(-1) // Higher priority
                .terminal(),
        );

        router.add_rule(
            RouteRule::new("all_backup")
                .with_collection("*")
                .with_sink("backup"),
        );

        // Products should only go to kafka (terminal)
        let event = create_test_event("products", Operation::Insert);
        let decision = router.route(event);
        assert_eq!(decision.matched_rules.len(), 1);
        assert!(decision.sink_ids.contains(&"kafka".to_string()));
        assert!(!decision.sink_ids.contains(&"backup".to_string()));

        // Users should go to backup
        let event = create_test_event("users", Operation::Insert);
        let decision = router.route(event);
        assert!(decision.sink_ids.contains(&"backup".to_string()));
    }

    #[test]
    fn test_router_default_sinks() {
        let router = EventRouter::new();

        router.add_rule(
            RouteRule::new("products_only")
                .with_collection("products")
                .with_sink("kafka"),
        );

        router.set_default_sinks(vec!["backup".to_string()]);

        // Products go to kafka
        let event = create_test_event("products", Operation::Insert);
        let decision = router.route(event);
        assert!(decision.sink_ids.contains(&"kafka".to_string()));

        // Others go to default
        let event = create_test_event("unknown", Operation::Insert);
        let decision = router.route(event);
        assert!(decision.sink_ids.contains(&"backup".to_string()));
    }

    #[test]
    fn test_router_stats() {
        let router = EventRouter::new();
        router.add_rule(
            RouteRule::new("all")
                .with_collection("*")
                .with_sink("kafka"),
        );

        router.route(create_test_event("a", Operation::Insert));
        router.route(create_test_event("b", Operation::Insert));

        let stats = router.stats();
        assert_eq!(stats.events_routed, 2);
        assert_eq!(stats.events_matched, 2);
        assert_eq!(*stats.rule_matches.get("all").unwrap(), 2);
    }

    #[test]
    fn test_enable_disable_rule() {
        let router = EventRouter::new();
        router.add_rule(
            RouteRule::new("test")
                .with_collection("*")
                .with_sink("kafka"),
        );

        // Initially enabled
        let event = create_test_event("any", Operation::Insert);
        let decision = router.route(event);
        assert!(decision.has_match);

        // Disable rule
        router.set_rule_enabled("test", false);
        let event = create_test_event("any", Operation::Insert);
        let decision = router.route(event);
        assert!(!decision.has_match);

        // Re-enable
        router.set_rule_enabled("test", true);
        let event = create_test_event("any", Operation::Insert);
        let decision = router.route(event);
        assert!(decision.has_match);
    }
}
