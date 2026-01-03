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

//! Outbound CDC configuration

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Configuration for outbound CDC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundConfig {
    /// Subscription name (unique identifier)
    pub name: String,
    /// Collections to subscribe to (empty = all)
    pub collections: HashSet<String>,
    /// Start position
    #[serde(default)]
    pub start_position: StartPosition,
    /// Enable exactly-once delivery
    #[serde(default)]
    pub exactly_once: bool,
    /// Deduplication window size
    #[serde(default = "default_dedup_size")]
    pub dedup_cache_size: usize,
    /// Batch size for reading WAL entries
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Poll interval when no events
    #[serde(default = "default_poll_interval")]
    pub poll_interval_ms: u64,
    /// Acknowledgment timeout
    #[serde(default = "default_ack_timeout")]
    pub ack_timeout_ms: u64,
    /// Maximum unacknowledged events
    #[serde(default = "default_max_unacked")]
    pub max_unacked_events: usize,
    /// Enable automatic checkpointing
    #[serde(default = "default_true")]
    pub auto_checkpoint: bool,
    /// Checkpoint interval
    #[serde(default = "default_checkpoint_interval")]
    pub checkpoint_interval_ms: u64,
}

fn default_dedup_size() -> usize {
    10000
}

fn default_batch_size() -> usize {
    100
}

fn default_poll_interval() -> u64 {
    100 // 100ms
}

fn default_ack_timeout() -> u64 {
    30000 // 30 seconds
}

fn default_max_unacked() -> usize {
    1000
}

fn default_true() -> bool {
    true
}

fn default_checkpoint_interval() -> u64 {
    5000 // 5 seconds
}

/// Starting position for subscription
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StartPosition {
    /// Start from the beginning of the WAL
    Beginning,
    /// Start from the current position (new events only)
    #[default]
    Latest,
    /// Start from a specific LSN
    Lsn(u64),
    /// Start from a specific timestamp
    Timestamp(u64),
    /// Resume from last checkpoint
    Resume,
}

impl Default for OutboundConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl OutboundConfig {
    /// Create a new outbound configuration
    pub fn new() -> Self {
        Self {
            name: "default".to_string(),
            collections: HashSet::new(),
            start_position: StartPosition::Latest,
            exactly_once: false,
            dedup_cache_size: default_dedup_size(),
            batch_size: default_batch_size(),
            poll_interval_ms: default_poll_interval(),
            ack_timeout_ms: default_ack_timeout(),
            max_unacked_events: default_max_unacked(),
            auto_checkpoint: true,
            checkpoint_interval_ms: default_checkpoint_interval(),
        }
    }

    /// Set subscription name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Add a collection to subscribe to
    pub fn with_collection(mut self, collection: impl Into<String>) -> Self {
        self.collections.insert(collection.into());
        self
    }

    /// Add multiple collections
    pub fn with_collections(mut self, collections: Vec<impl Into<String>>) -> Self {
        for c in collections {
            self.collections.insert(c.into());
        }
        self
    }

    /// Set start position
    pub fn with_start_position(mut self, position: StartPosition) -> Self {
        self.start_position = position;
        self
    }

    /// Start from beginning
    pub fn from_beginning(mut self) -> Self {
        self.start_position = StartPosition::Beginning;
        self
    }

    /// Start from specific LSN
    pub fn from_lsn(mut self, lsn: u64) -> Self {
        self.start_position = StartPosition::Lsn(lsn);
        self
    }

    /// Enable exactly-once delivery
    pub fn with_exactly_once(mut self, enabled: bool) -> Self {
        self.exactly_once = enabled;
        self
    }

    /// Set batch size
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Set poll interval
    pub fn with_poll_interval(mut self, ms: u64) -> Self {
        self.poll_interval_ms = ms;
        self
    }

    /// Set ack timeout
    pub fn with_ack_timeout(mut self, ms: u64) -> Self {
        self.ack_timeout_ms = ms;
        self
    }

    /// Get poll interval as Duration
    pub fn poll_interval(&self) -> Duration {
        Duration::from_millis(self.poll_interval_ms)
    }

    /// Get ack timeout as Duration
    pub fn ack_timeout(&self) -> Duration {
        Duration::from_millis(self.ack_timeout_ms)
    }

    /// Check if should subscribe to a collection
    pub fn should_include(&self, collection: &str) -> bool {
        self.collections.is_empty() || self.collections.contains(collection)
    }
}

/// Configuration for a subscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionConfig {
    /// Subscription ID
    pub id: String,
    /// Outbound configuration
    pub config: OutboundConfig,
    /// Routes for this subscription
    pub routes: Vec<RouteConfig>,
}

impl SubscriptionConfig {
    /// Create a new subscription config
    pub fn new(id: impl Into<String>, config: OutboundConfig) -> Self {
        Self {
            id: id.into(),
            config,
            routes: Vec::new(),
        }
    }

    /// Add a route
    pub fn with_route(mut self, route: RouteConfig) -> Self {
        self.routes.push(route);
        self
    }
}

/// Configuration for event routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    /// Route name
    pub name: String,
    /// Sink type
    pub sink_type: String,
    /// Collection patterns to match
    pub patterns: Vec<String>,
    /// Sink-specific configuration
    pub sink_config: serde_json::Value,
    /// Priority (lower = higher priority)
    #[serde(default)]
    pub priority: u32,
    /// Enable this route
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl RouteConfig {
    /// Create a new route config
    pub fn new(name: impl Into<String>, sink_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sink_type: sink_type.into(),
            patterns: Vec::new(),
            sink_config: serde_json::Value::Null,
            priority: 0,
            enabled: true,
        }
    }

    /// Add a pattern
    pub fn with_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.patterns.push(pattern.into());
        self
    }

    /// Set sink config
    pub fn with_sink_config(mut self, config: serde_json::Value) -> Self {
        self.sink_config = config;
        self
    }

    /// Set priority
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// Check if pattern matches collection
    pub fn matches(&self, collection: &str) -> bool {
        if self.patterns.is_empty() {
            return true;
        }

        for pattern in &self.patterns {
            if pattern == "*" || pattern == collection {
                return true;
            }
            if pattern.ends_with('*') {
                let prefix = &pattern[..pattern.len() - 1];
                if collection.starts_with(prefix) {
                    return true;
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outbound_config_default() {
        let config = OutboundConfig::new();
        assert_eq!(config.name, "default");
        assert!(config.collections.is_empty());
        assert!(matches!(config.start_position, StartPosition::Latest));
        assert!(!config.exactly_once);
    }

    #[test]
    fn test_outbound_config_builder() {
        let config = OutboundConfig::new()
            .with_name("my_sub")
            .with_collection("products")
            .with_collection("users")
            .with_exactly_once(true)
            .with_batch_size(50);

        assert_eq!(config.name, "my_sub");
        assert!(config.collections.contains("products"));
        assert!(config.collections.contains("users"));
        assert!(config.exactly_once);
        assert_eq!(config.batch_size, 50);
    }

    #[test]
    fn test_start_positions() {
        let config = OutboundConfig::new().from_beginning();
        assert!(matches!(config.start_position, StartPosition::Beginning));

        let config = OutboundConfig::new().from_lsn(12345);
        assert!(matches!(config.start_position, StartPosition::Lsn(12345)));
    }

    #[test]
    fn test_should_include() {
        let config = OutboundConfig::new();
        // Empty collections means include all
        assert!(config.should_include("anything"));

        let config = OutboundConfig::new()
            .with_collection("products")
            .with_collection("users");
        assert!(config.should_include("products"));
        assert!(config.should_include("users"));
        assert!(!config.should_include("orders"));
    }

    #[test]
    fn test_route_config() {
        let route = RouteConfig::new("kafka_route", "kafka")
            .with_pattern("products*")
            .with_priority(1);

        assert_eq!(route.name, "kafka_route");
        assert_eq!(route.sink_type, "kafka");
        assert!(route.matches("products"));
        assert!(route.matches("products_v2"));
        assert!(!route.matches("users"));
    }

    #[test]
    fn test_route_wildcard_matching() {
        let route = RouteConfig::new("all", "kafka").with_pattern("*");
        assert!(route.matches("anything"));

        let route = RouteConfig::new("prefix", "kafka").with_pattern("public.*");
        assert!(route.matches("public.users"));
        assert!(route.matches("public.products"));
        assert!(!route.matches("private.secrets"));
    }

    #[test]
    fn test_subscription_config() {
        let sub =
            SubscriptionConfig::new("sub1", OutboundConfig::new().with_collection("products"))
                .with_route(RouteConfig::new("r1", "kafka"))
                .with_route(RouteConfig::new("r2", "webhook"));

        assert_eq!(sub.id, "sub1");
        assert_eq!(sub.routes.len(), 2);
    }

    #[test]
    fn test_durations() {
        let config = OutboundConfig::new()
            .with_poll_interval(500)
            .with_ack_timeout(60000);

        assert_eq!(config.poll_interval(), Duration::from_millis(500));
        assert_eq!(config.ack_timeout(), Duration::from_secs(60));
    }
}
