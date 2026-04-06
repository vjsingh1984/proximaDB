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

//! Stream configuration types
//!
//! This module provides configuration types for streaming operations,
//! including global coordinator settings and per-session settings.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Ordering guarantees for stream processing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderingMode {
    /// No ordering guarantees (fastest, highest throughput)
    #[default]
    Unordered,

    /// Ordered within the same partition key
    PartitionOrdered,

    /// Globally ordered across all partitions (slowest)
    GloballyOrdered,
}

/// Delivery semantics for stream processing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliverySemantics {
    /// Records may be lost on failure (fastest)
    AtMostOnce,

    /// Records may be duplicated on failure (safe default)
    #[default]
    AtLeastOnce,

    /// Exactly one delivery (requires 2PC, slowest)
    ExactlyOnce,
}

/// Global stream coordinator configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Maximum number of concurrent streaming sessions
    #[serde(default = "default_max_streams")]
    pub max_streams: usize,

    /// Default buffer size per stream (in records)
    #[serde(default = "default_buffer_size")]
    pub default_buffer_size: usize,

    /// Global rate limit (records per second, 0 = unlimited)
    #[serde(default = "default_rate_limit")]
    pub global_rate_limit: u64,

    /// Default flush interval for persisting buffered records
    #[serde(default = "default_flush_interval", with = "duration_millis")]
    pub flush_interval: Duration,

    /// Session timeout (inactive sessions are closed)
    #[serde(default = "default_session_timeout", with = "duration_secs")]
    pub session_timeout: Duration,

    /// Backpressure configuration
    #[serde(default)]
    pub backpressure: BackpressureConfig,
}

/// Backpressure configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackpressureConfig {
    /// Low watermark ratio (0.0 - 1.0)
    #[serde(default = "default_low_watermark")]
    pub low_watermark: f32,

    /// High watermark ratio (0.0 - 1.0)
    #[serde(default = "default_high_watermark")]
    pub high_watermark: f32,

    /// Critical watermark ratio (0.0 - 1.0)
    #[serde(default = "default_critical_watermark")]
    pub critical_watermark: f32,

    /// Minimum backoff delay in milliseconds
    #[serde(default = "default_min_delay_ms")]
    pub min_delay_ms: u32,

    /// Maximum backoff delay in milliseconds
    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u32,

    /// Backoff multiplier for exponential backoff
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f32,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            low_watermark: default_low_watermark(),
            high_watermark: default_high_watermark(),
            critical_watermark: default_critical_watermark(),
            min_delay_ms: default_min_delay_ms(),
            max_delay_ms: default_max_delay_ms(),
            backoff_multiplier: default_backoff_multiplier(),
        }
    }
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            max_streams: default_max_streams(),
            default_buffer_size: default_buffer_size(),
            global_rate_limit: default_rate_limit(),
            flush_interval: Duration::from_millis(default_flush_interval_ms()),
            session_timeout: Duration::from_secs(default_session_timeout_secs()),
            backpressure: BackpressureConfig::default(),
        }
    }
}

/// Per-session configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionConfig {
    /// Buffer size for this session (overrides default)
    pub buffer_size: Option<usize>,

    /// Rate limit for this session (records/second, overrides global)
    pub rate_limit: Option<u64>,

    /// Ordering mode for this session
    #[serde(default)]
    pub ordering: OrderingMode,

    /// Delivery semantics for this session
    #[serde(default)]
    pub delivery: DeliverySemantics,

    /// Partition key for ordered delivery
    pub partition_key: Option<String>,

    /// Flush interval for this session (overrides default)
    #[serde(default, with = "option_duration_millis")]
    pub flush_interval: Option<Duration>,
}

impl SessionConfig {
    /// Create a session config with a specific buffer size
    pub fn with_buffer_size(size: usize) -> Self {
        Self {
            buffer_size: Some(size),
            ..Default::default()
        }
    }

    /// Create a session config with exactly-once semantics
    pub fn exactly_once() -> Self {
        Self {
            delivery: DeliverySemantics::ExactlyOnce,
            ordering: OrderingMode::GloballyOrdered,
            ..Default::default()
        }
    }

    /// Create a session config optimized for high throughput
    pub fn high_throughput() -> Self {
        Self {
            ordering: OrderingMode::Unordered,
            delivery: DeliverySemantics::AtMostOnce,
            buffer_size: Some(100_000),
            ..Default::default()
        }
    }
}

// Default value functions
fn default_max_streams() -> usize {
    1000
}

fn default_buffer_size() -> usize {
    10_000
}

fn default_rate_limit() -> u64 {
    1_000_000 // 1M records/sec
}

fn default_flush_interval_ms() -> u64 {
    100
}

fn default_flush_interval() -> Duration {
    Duration::from_millis(100)
}

fn default_session_timeout_secs() -> u64 {
    300 // 5 minutes
}

fn default_session_timeout() -> Duration {
    Duration::from_secs(300)
}

fn default_low_watermark() -> f32 {
    0.25
}

fn default_high_watermark() -> f32 {
    0.75
}

fn default_critical_watermark() -> f32 {
    0.90
}

fn default_min_delay_ms() -> u32 {
    10
}

fn default_max_delay_ms() -> u32 {
    1000
}

fn default_backoff_multiplier() -> f32 {
    2.0
}

// Serde helpers for Duration
mod duration_millis {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_millis() as u64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

mod option_duration_millis {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match duration {
            Some(d) => serializer.serialize_some(&(d.as_millis() as u64)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<u64>::deserialize(deserializer)?;
        Ok(opt.map(Duration::from_millis))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_stream_config() {
        let config = StreamConfig::default();
        assert_eq!(config.max_streams, 1000);
        assert_eq!(config.default_buffer_size, 10_000);
        assert_eq!(config.global_rate_limit, 1_000_000);
        assert_eq!(config.flush_interval, Duration::from_millis(100));
    }

    #[test]
    fn test_session_config_presets() {
        let exactly_once = SessionConfig::exactly_once();
        assert_eq!(exactly_once.delivery, DeliverySemantics::ExactlyOnce);
        assert_eq!(exactly_once.ordering, OrderingMode::GloballyOrdered);

        let high_throughput = SessionConfig::high_throughput();
        assert_eq!(high_throughput.delivery, DeliverySemantics::AtMostOnce);
        assert_eq!(high_throughput.ordering, OrderingMode::Unordered);
        assert_eq!(high_throughput.buffer_size, Some(100_000));
    }

    #[test]
    fn test_config_serialization() {
        let config = StreamConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: StreamConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.max_streams, parsed.max_streams);
    }
}
