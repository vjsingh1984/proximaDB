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

//! Filter rules for CDC events

use serde::{Deserialize, Serialize};

use crate::cdc::event::{ChangeEvent, ConnectorType, Operation};

/// Action to take after filter evaluation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    /// Include the event
    Include,
    /// Exclude the event
    Exclude,
    /// Continue to next filter
    Continue,
}

/// A set of filter rules with logical combination
#[derive(Debug, Clone, Default)]
pub struct FilterRuleSet {
    /// Filter rules
    rules: Vec<FilterRule>,
    /// Combination mode
    mode: FilterMode,
}

/// How to combine filter rules
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    /// All rules must match (AND)
    #[default]
    All,
    /// Any rule can match (OR)
    Any,
    /// None of the rules should match (NOT)
    None,
}

impl FilterRuleSet {
    /// Create a new filter rule set
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with AND mode
    pub fn all() -> Self {
        Self {
            rules: Vec::new(),
            mode: FilterMode::All,
        }
    }

    /// Create with OR mode
    pub fn any() -> Self {
        Self {
            rules: Vec::new(),
            mode: FilterMode::Any,
        }
    }

    /// Create with NOT mode
    pub fn none() -> Self {
        Self {
            rules: Vec::new(),
            mode: FilterMode::None,
        }
    }

    /// Add a rule to the set
    pub fn with_rule(mut self, rule: FilterRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Add multiple rules
    pub fn with_rules(mut self, rules: Vec<FilterRule>) -> Self {
        self.rules.extend(rules);
        self
    }

    /// Set the filter mode
    pub fn with_mode(mut self, mode: FilterMode) -> Self {
        self.mode = mode;
        self
    }

    /// Evaluate the filter set against an event
    pub fn evaluate(&self, event: &ChangeEvent) -> FilterAction {
        if self.rules.is_empty() {
            return FilterAction::Continue;
        }

        match self.mode {
            FilterMode::All => {
                // All rules must pass
                for rule in &self.rules {
                    if !rule.matches(event) {
                        return FilterAction::Exclude;
                    }
                }
                FilterAction::Include
            }
            FilterMode::Any => {
                // At least one rule must pass
                for rule in &self.rules {
                    if rule.matches(event) {
                        return FilterAction::Include;
                    }
                }
                FilterAction::Exclude
            }
            FilterMode::None => {
                // No rules should pass
                for rule in &self.rules {
                    if rule.matches(event) {
                        return FilterAction::Exclude;
                    }
                }
                FilterAction::Include
            }
        }
    }

    /// Check if the rule set is empty
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Get the number of rules
    pub fn len(&self) -> usize {
        self.rules.len()
    }
}

/// A single filter rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterRule {
    /// Rule type
    rule_type: FilterRuleType,
    /// Whether to negate the rule
    negate: bool,
}

/// Types of filter rules
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FilterRuleType {
    /// Filter by collection names
    Collection { patterns: Vec<String> },
    /// Filter by operation types
    Operation { operations: Vec<Operation> },
    /// Filter by connector type
    Connector { connectors: Vec<ConnectorType> },
    /// Filter by key pattern
    Key { patterns: Vec<String> },
    /// Filter by metadata field value
    Metadata {
        field: String,
        condition: MetadataCondition,
    },
    /// Filter by LSN range
    LsnRange { min: Option<u64>, max: Option<u64> },
    /// Composite rule (nested rules)
    Composite {
        rules: Vec<FilterRule>,
        mode: FilterMode,
    },
    /// Always match (for testing)
    Always,
    /// Never match (for testing)
    Never,
}

/// Conditions for metadata filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MetadataCondition {
    /// Field exists
    Exists,
    /// Field equals value
    Equals { value: serde_json::Value },
    /// Field not equals value
    NotEquals { value: serde_json::Value },
    /// Field contains value (string/array)
    Contains { value: String },
    /// Field starts with value
    StartsWith { value: String },
    /// Field ends with value
    EndsWith { value: String },
    /// Field matches regex
    Matches { pattern: String },
    /// Field is greater than
    GreaterThan { value: f64 },
    /// Field is less than
    LessThan { value: f64 },
    /// Field is in range
    InRange { min: f64, max: f64 },
    /// Field is in list
    In { values: Vec<serde_json::Value> },
    /// Field is null
    IsNull,
    /// Field is not null
    IsNotNull,
}

impl FilterRule {
    /// Create a rule that includes matching collections
    pub fn include_collections(patterns: Vec<impl Into<String>>) -> Self {
        Self {
            rule_type: FilterRuleType::Collection {
                patterns: patterns.into_iter().map(|p| p.into()).collect(),
            },
            negate: false,
        }
    }

    /// Create a rule that excludes matching collections
    pub fn exclude_collections(patterns: Vec<impl Into<String>>) -> Self {
        Self {
            rule_type: FilterRuleType::Collection {
                patterns: patterns.into_iter().map(|p| p.into()).collect(),
            },
            negate: true,
        }
    }

    /// Create a rule that includes matching operations
    pub fn include_operations(operations: Vec<Operation>) -> Self {
        Self {
            rule_type: FilterRuleType::Operation { operations },
            negate: false,
        }
    }

    /// Create a rule that excludes matching operations
    pub fn exclude_operations(operations: Vec<Operation>) -> Self {
        Self {
            rule_type: FilterRuleType::Operation { operations },
            negate: true,
        }
    }

    /// Create a rule that includes matching connectors
    pub fn include_connectors(connectors: Vec<ConnectorType>) -> Self {
        Self {
            rule_type: FilterRuleType::Connector { connectors },
            negate: false,
        }
    }

    /// Create a rule that includes matching keys
    pub fn include_keys(patterns: Vec<impl Into<String>>) -> Self {
        Self {
            rule_type: FilterRuleType::Key {
                patterns: patterns.into_iter().map(|p| p.into()).collect(),
            },
            negate: false,
        }
    }

    /// Create a metadata filter rule
    pub fn metadata(field: impl Into<String>, condition: MetadataCondition) -> Self {
        Self {
            rule_type: FilterRuleType::Metadata {
                field: field.into(),
                condition,
            },
            negate: false,
        }
    }

    /// Create an LSN range filter
    pub fn lsn_range(min: Option<u64>, max: Option<u64>) -> Self {
        Self {
            rule_type: FilterRuleType::LsnRange { min, max },
            negate: false,
        }
    }

    /// Create an always-match rule
    pub fn always() -> Self {
        Self {
            rule_type: FilterRuleType::Always,
            negate: false,
        }
    }

    /// Create a never-match rule
    pub fn never() -> Self {
        Self {
            rule_type: FilterRuleType::Never,
            negate: false,
        }
    }

    /// Negate this rule
    pub fn not(mut self) -> Self {
        self.negate = !self.negate;
        self
    }

    /// Check if this rule matches an event
    pub fn matches(&self, event: &ChangeEvent) -> bool {
        let result = self.matches_inner(event);
        if self.negate { !result } else { result }
    }

    fn matches_inner(&self, event: &ChangeEvent) -> bool {
        match &self.rule_type {
            FilterRuleType::Collection { patterns } => patterns
                .iter()
                .any(|p| self.pattern_matches(p, &event.collection)),

            FilterRuleType::Operation { operations } => operations.contains(&event.operation),

            FilterRuleType::Connector { connectors } => {
                connectors.contains(&event.source.connector)
            }

            FilterRuleType::Key { patterns } => {
                patterns.iter().any(|p| self.pattern_matches(p, &event.key))
            }

            FilterRuleType::Metadata { field, condition } => {
                self.check_metadata(event, field, condition)
            }

            FilterRuleType::LsnRange { min, max } => {
                if let Some(min_lsn) = min
                    && event.lsn < *min_lsn {
                        return false;
                    }
                if let Some(max_lsn) = max
                    && event.lsn > *max_lsn {
                        return false;
                    }
                true
            }

            FilterRuleType::Composite { rules, mode } => {
                let ruleset = FilterRuleSet {
                    rules: rules.clone(),
                    mode: *mode,
                };
                matches!(ruleset.evaluate(event), FilterAction::Include)
            }

            FilterRuleType::Always => true,
            FilterRuleType::Never => false,
        }
    }

    /// Simple pattern matching with wildcards
    fn pattern_matches(&self, pattern: &str, value: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        if pattern.starts_with('*') && pattern.ends_with('*') {
            let inner = &pattern[1..pattern.len() - 1];
            return value.contains(inner);
        }

        if let Some(suffix) = pattern.strip_prefix('*') {
            return value.ends_with(suffix);
        }

        if let Some(prefix) = pattern.strip_suffix('*') {
            return value.starts_with(prefix);
        }

        pattern == value
    }

    /// Check metadata condition
    fn check_metadata(
        &self,
        event: &ChangeEvent,
        field: &str,
        condition: &MetadataCondition,
    ) -> bool {
        // Get metadata from after state (or before for deletes)
        let metadata = if let Some(ref after) = event.after {
            &after.metadata
        } else if let Some(ref before) = event.before {
            &before.metadata
        } else {
            return false;
        };

        let value = metadata.get(field);

        match condition {
            MetadataCondition::Exists => value.is_some(),

            MetadataCondition::Equals { value: expected } => value == Some(expected),

            MetadataCondition::NotEquals { value: expected } => value != Some(expected),

            MetadataCondition::Contains { value: search } => value
                .is_some_and(|v| match v {
                    serde_json::Value::String(s) => s.contains(search),
                    serde_json::Value::Array(arr) => arr
                        .iter()
                        .any(|item| item.as_str().is_some_and(|s| s == search)),
                    _ => false,
                }),

            MetadataCondition::StartsWith { value: prefix } => value
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.starts_with(prefix)),

            MetadataCondition::EndsWith { value: suffix } => value
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.ends_with(suffix)),

            MetadataCondition::Matches { pattern } => {
                // Simple substring match (would use regex crate in production)
                value
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.contains(pattern))
            }

            MetadataCondition::GreaterThan { value: threshold } => value
                .and_then(|v| v.as_f64())
                .is_some_and(|n| n > *threshold),

            MetadataCondition::LessThan { value: threshold } => value
                .and_then(|v| v.as_f64())
                .is_some_and(|n| n < *threshold),

            MetadataCondition::InRange { min, max } => value
                .and_then(|v| v.as_f64())
                .is_some_and(|n| n >= *min && n <= *max),

            MetadataCondition::In { values } => value.is_some_and(|v| values.contains(v)),

            MetadataCondition::IsNull => value.is_none_or(|v| v.is_null()),

            MetadataCondition::IsNotNull => value.is_some_and(|v| !v.is_null()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::event::{RecordState, SourceInfo};
    use std::collections::HashMap;

    fn create_test_event() -> ChangeEvent {
        let mut metadata = HashMap::new();
        metadata.insert("name".to_string(), serde_json::json!("John"));
        metadata.insert("age".to_string(), serde_json::json!(30));
        metadata.insert("email".to_string(), serde_json::json!("john@example.com"));

        let mut event = ChangeEvent::new(
            SourceInfo::postgres("testdb", "public", "test_server"),
            Operation::Insert,
            "public.users",
            "user_123",
        );
        event.after = Some(RecordState {
            vector: None,
            metadata,
            raw: None,
        });
        event
    }

    #[test]
    fn test_filter_rule_set_empty() {
        let ruleset = FilterRuleSet::new();
        assert!(ruleset.is_empty());

        let event = create_test_event();
        assert_eq!(ruleset.evaluate(&event), FilterAction::Continue);
    }

    #[test]
    fn test_include_collections() {
        let rule = FilterRule::include_collections(vec!["public.users"]);
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_include_collections_wildcard() {
        let rule = FilterRule::include_collections(vec!["public.*"]);
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_exclude_collections() {
        let rule = FilterRule::exclude_collections(vec!["public.users"]);
        let event = create_test_event();
        assert!(!rule.matches(&event));
    }

    #[test]
    fn test_include_operations() {
        let rule = FilterRule::include_operations(vec![Operation::Insert]);
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_exclude_operations() {
        let rule = FilterRule::exclude_operations(vec![Operation::Delete]);
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_include_keys() {
        let rule = FilterRule::include_keys(vec!["user_*"]);
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_metadata_exists() {
        let rule = FilterRule::metadata("name", MetadataCondition::Exists);
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_metadata_not_exists() {
        let rule = FilterRule::metadata("missing", MetadataCondition::Exists);
        let event = create_test_event();
        assert!(!rule.matches(&event));
    }

    #[test]
    fn test_metadata_equals() {
        let rule = FilterRule::metadata(
            "name",
            MetadataCondition::Equals {
                value: serde_json::json!("John"),
            },
        );
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_metadata_greater_than() {
        let rule = FilterRule::metadata("age", MetadataCondition::GreaterThan { value: 25.0 });
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_metadata_less_than() {
        let rule = FilterRule::metadata("age", MetadataCondition::LessThan { value: 35.0 });
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_metadata_in_range() {
        let rule = FilterRule::metadata(
            "age",
            MetadataCondition::InRange {
                min: 20.0,
                max: 40.0,
            },
        );
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_metadata_contains() {
        let rule = FilterRule::metadata(
            "email",
            MetadataCondition::Contains {
                value: "example".to_string(),
            },
        );
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_metadata_starts_with() {
        let rule = FilterRule::metadata(
            "email",
            MetadataCondition::StartsWith {
                value: "john".to_string(),
            },
        );
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_lsn_range() {
        let rule = FilterRule::lsn_range(Some(0), Some(1000));
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_rule_negation() {
        let rule = FilterRule::include_collections(vec!["public.users"]).not();
        let event = create_test_event();
        assert!(!rule.matches(&event));
    }

    #[test]
    fn test_filter_mode_all() {
        let ruleset = FilterRuleSet::all()
            .with_rule(FilterRule::include_operations(vec![Operation::Insert]))
            .with_rule(FilterRule::include_collections(vec!["public.users"]));

        let event = create_test_event();
        assert_eq!(ruleset.evaluate(&event), FilterAction::Include);
    }

    #[test]
    fn test_filter_mode_all_fail() {
        let ruleset = FilterRuleSet::all()
            .with_rule(FilterRule::include_operations(vec![Operation::Update]))
            .with_rule(FilterRule::include_collections(vec!["public.users"]));

        let event = create_test_event();
        assert_eq!(ruleset.evaluate(&event), FilterAction::Exclude);
    }

    #[test]
    fn test_filter_mode_any() {
        let ruleset = FilterRuleSet::any()
            .with_rule(FilterRule::include_operations(vec![Operation::Update]))
            .with_rule(FilterRule::include_collections(vec!["public.users"]));

        let event = create_test_event();
        assert_eq!(ruleset.evaluate(&event), FilterAction::Include);
    }

    #[test]
    fn test_filter_mode_none() {
        let ruleset = FilterRuleSet::none()
            .with_rule(FilterRule::include_operations(vec![Operation::Delete]))
            .with_rule(FilterRule::include_collections(vec!["public.orders"]));

        let event = create_test_event();
        assert_eq!(ruleset.evaluate(&event), FilterAction::Include);
    }

    #[test]
    fn test_always_rule() {
        let rule = FilterRule::always();
        let event = create_test_event();
        assert!(rule.matches(&event));
    }

    #[test]
    fn test_never_rule() {
        let rule = FilterRule::never();
        let event = create_test_event();
        assert!(!rule.matches(&event));
    }

    #[test]
    fn test_wildcard_patterns() {
        let rule = FilterRule::include_collections(vec!["*"]);
        let event = create_test_event();
        assert!(rule.matches(&event));

        let rule2 = FilterRule::include_collections(vec!["*users"]);
        assert!(rule2.matches(&event));

        let rule3 = FilterRule::include_collections(vec!["*lic*"]);
        assert!(rule3.matches(&event));
    }
}
