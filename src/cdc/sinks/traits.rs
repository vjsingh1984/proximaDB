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

//! Sink traits and common types

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::cdc::event::ChangeEvent;

use super::{KafkaConfig, WebhookConfig};

/// Result type for sink operations
pub type SinkResult<T> = Result<T, SinkError>;

/// Error type for sink operations
#[derive(Debug, Clone)]
pub enum SinkError {
    /// Connection error
    Connection(String),
    /// Send error
    Send(String),
    /// Configuration error
    Configuration(String),
    /// Serialization error
    Serialization(String),
    /// Timeout error
    Timeout(String),
    /// Authentication error
    Authentication(String),
    /// Rate limit error
    RateLimit {
        /// Retry after (seconds)
        retry_after: Option<u64>,
    },
    /// Other error
    Other(String),
}

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(msg) => write!(f, "Connection error: {}", msg),
            Self::Send(msg) => write!(f, "Send error: {}", msg),
            Self::Configuration(msg) => write!(f, "Configuration error: {}", msg),
            Self::Serialization(msg) => write!(f, "Serialization error: {}", msg),
            Self::Timeout(msg) => write!(f, "Timeout: {}", msg),
            Self::Authentication(msg) => write!(f, "Authentication error: {}", msg),
            Self::RateLimit { retry_after } => {
                if let Some(secs) = retry_after {
                    write!(f, "Rate limited, retry after {} seconds", secs)
                } else {
                    write!(f, "Rate limited")
                }
            }
            Self::Other(msg) => write!(f, "Error: {}", msg),
        }
    }
}

impl std::error::Error for SinkError {}

/// Trait for CDC sinks
#[async_trait::async_trait]
pub trait CdcSink: Send + Sync {
    /// Get the sink name
    fn name(&self) -> &str;

    /// Send a single event
    async fn send(&self, event: ChangeEvent) -> SinkResult<()>;

    /// Send a batch of events
    async fn send_batch(&self, events: Vec<ChangeEvent>) -> SinkResult<()> {
        for event in events {
            self.send(event).await?;
        }
        Ok(())
    }

    /// Flush any buffered events
    async fn flush(&self) -> SinkResult<()>;

    /// Close the sink
    async fn close(&self) -> SinkResult<()>;

    /// Get sink statistics
    fn stats(&self) -> SinkStats;
}

/// Configuration for sinks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SinkConfig {
    /// Sink name
    pub name: String,
    /// Sink type
    pub sink_type: String,
    /// Kafka configuration (if type is "kafka")
    pub kafka: Option<KafkaConfig>,
    /// Webhook configuration (if type is "webhook")
    pub webhook: Option<WebhookConfig>,
    /// Buffer configuration
    #[serde(default)]
    pub buffer: BufferConfig,
    /// Retry configuration
    #[serde(default)]
    pub retry: CdcSinkRetryConfig,
}

/// Buffer configuration for sinks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferConfig {
    /// Maximum buffer size
    #[serde(default = "default_buffer_size")]
    pub max_size: usize,
    /// Flush interval in milliseconds
    #[serde(default = "default_flush_interval")]
    pub flush_interval_ms: u64,
    /// Enable compression in buffer
    #[serde(default)]
    pub compression: bool,
}

fn default_buffer_size() -> usize {
    1000
}

fn default_flush_interval() -> u64 {
    100
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            max_size: default_buffer_size(),
            flush_interval_ms: default_flush_interval(),
            compression: false,
        }
    }
}

/// Backwards-compat alias for [`CdcSinkRetryConfig`].
pub type RetryConfig = CdcSinkRetryConfig;

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdcSinkRetryConfig {
    /// Maximum retry attempts
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Initial backoff in milliseconds
    #[serde(default = "default_initial_backoff")]
    pub initial_backoff_ms: u64,
    /// Maximum backoff in milliseconds
    #[serde(default = "default_max_backoff")]
    pub max_backoff_ms: u64,
    /// Backoff multiplier
    #[serde(default = "default_multiplier")]
    pub multiplier: f64,
    /// Jitter factor (0.0 - 1.0)
    #[serde(default = "default_jitter")]
    pub jitter: f64,
}

fn default_max_retries() -> u32 {
    3
}

fn default_initial_backoff() -> u64 {
    100
}

fn default_max_backoff() -> u64 {
    10000
}

fn default_multiplier() -> f64 {
    2.0
}

fn default_jitter() -> f64 {
    0.1
}

impl Default for CdcSinkRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            initial_backoff_ms: default_initial_backoff(),
            max_backoff_ms: default_max_backoff(),
            multiplier: default_multiplier(),
            jitter: default_jitter(),
        }
    }
}

impl CdcSinkRetryConfig {
    /// Create a new retry config
    pub fn new() -> Self {
        Self::default()
    }

    /// Set max retries
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Set initial backoff
    pub fn with_initial_backoff(mut self, ms: u64) -> Self {
        self.initial_backoff_ms = ms;
        self
    }

    /// Set max backoff
    pub fn with_max_backoff(mut self, ms: u64) -> Self {
        self.max_backoff_ms = ms;
        self
    }

    /// Calculate backoff for a given attempt
    pub fn backoff_for_attempt(&self, attempt: u32) -> u64 {
        let backoff = self.initial_backoff_ms as f64 * self.multiplier.powi(attempt as i32);
        let backoff = backoff.min(self.max_backoff_ms as f64);

        // Add jitter
        let jitter_range = backoff * self.jitter;
        let jitter_offset = (rand_simple() * 2.0 - 1.0) * jitter_range;

        (backoff + jitter_offset).max(0.0) as u64
    }
}

/// Simple random number generator (for jitter)
fn rand_simple() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or_default();
    nanos as f64 / u32::MAX as f64
}

/// Statistics for a sink
#[derive(Debug, Clone, Default)]
pub struct SinkStats {
    /// Number of events sent
    pub events_sent: u64,
    /// Number of bytes sent
    pub bytes_sent: u64,
    /// Number of batches sent
    pub batches_sent: u64,
    /// Number of errors
    pub errors: u64,
    /// Number of retries
    pub retries: u64,
    /// Average latency in milliseconds
    pub avg_latency_ms: f64,
    /// Last send timestamp
    pub last_send_time: Option<u64>,
}

impl SinkStats {
    /// Record a successful send
    pub fn record_send(&mut self, bytes: u64, latency_ms: f64) {
        self.events_sent += 1;
        self.bytes_sent += bytes;

        // Update average latency using running average
        let n = self.events_sent as f64;
        self.avg_latency_ms = self.avg_latency_ms * ((n - 1.0) / n) + latency_ms / n;

        self.last_send_time = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
        );
    }

    /// Record a batch send
    pub fn record_batch(&mut self, count: u64, bytes: u64, latency_ms: f64) {
        self.events_sent += count;
        self.bytes_sent += bytes;
        self.batches_sent += 1;

        let n = self.batches_sent as f64;
        self.avg_latency_ms = self.avg_latency_ms * ((n - 1.0) / n) + latency_ms / n;

        self.last_send_time = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
        );
    }

    /// Record an error
    pub fn record_error(&mut self) {
        self.errors += 1;
    }

    /// Record a retry
    pub fn record_retry(&mut self) {
        self.retries += 1;
    }
}

/// Message format for sink output
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MessageFormat {
    /// JSON format
    #[default]
    Json,
    /// Avro format
    Avro,
    /// Protocol Buffers
    Protobuf,
    /// MessagePack
    MessagePack,
}

impl MessageFormat {
    /// Serialize an event to bytes
    pub fn serialize(&self, event: &ChangeEvent) -> SinkResult<Vec<u8>> {
        match self {
            Self::Json => {
                serde_json::to_vec(event).map_err(|e| SinkError::Serialization(e.to_string()))
            }
            Self::Avro => {
                // Placeholder - would use apache-avro in production
                serde_json::to_vec(event).map_err(|e| SinkError::Serialization(e.to_string()))
            }
            Self::Protobuf => {
                // Placeholder - would use prost in production
                serde_json::to_vec(event).map_err(|e| SinkError::Serialization(e.to_string()))
            }
            Self::MessagePack => {
                // Placeholder - would use rmp-serde in production
                serde_json::to_vec(event).map_err(|e| SinkError::Serialization(e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sink_error_display() {
        let err = SinkError::Connection("Failed to connect".to_string());
        assert!(err.to_string().contains("Connection error"));

        let err = SinkError::RateLimit {
            retry_after: Some(60),
        };
        assert!(err.to_string().contains("60 seconds"));
    }

    #[test]
    fn test_buffer_config_default() {
        let config = BufferConfig::default();
        assert_eq!(config.max_size, 1000);
        assert_eq!(config.flush_interval_ms, 100);
        assert!(!config.compression);
    }

    #[test]
    fn test_retry_config_default() {
        let config = CdcSinkRetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_backoff_ms, 100);
        assert_eq!(config.max_backoff_ms, 10000);
    }

    #[test]
    fn test_retry_backoff_calculation() {
        let config = CdcSinkRetryConfig::new()
            .with_initial_backoff(100)
            .with_max_retries(5);

        let backoff0 = config.backoff_for_attempt(0);
        let backoff1 = config.backoff_for_attempt(1);
        let backoff2 = config.backoff_for_attempt(2);

        // Backoff should increase (approximately, due to jitter)
        assert!(backoff1 > backoff0 / 2);
        assert!(backoff2 > backoff1 / 2);
    }

    #[test]
    fn test_retry_max_backoff() {
        let config = CdcSinkRetryConfig::new()
            .with_initial_backoff(100)
            .with_max_backoff(500);

        let backoff = config.backoff_for_attempt(10);
        // Should be capped near max (with some jitter)
        assert!(backoff <= 600);
    }

    #[test]
    fn test_sink_stats() {
        let mut stats = SinkStats::default();

        stats.record_send(1024, 10.0);
        assert_eq!(stats.events_sent, 1);
        assert_eq!(stats.bytes_sent, 1024);
        assert_eq!(stats.avg_latency_ms, 10.0);

        stats.record_send(512, 20.0);
        assert_eq!(stats.events_sent, 2);
        assert_eq!(stats.bytes_sent, 1536);
        assert_eq!(stats.avg_latency_ms, 15.0);
    }

    #[test]
    fn test_sink_stats_batch() {
        let mut stats = SinkStats::default();

        stats.record_batch(10, 5000, 50.0);
        assert_eq!(stats.events_sent, 10);
        assert_eq!(stats.bytes_sent, 5000);
        assert_eq!(stats.batches_sent, 1);
    }

    #[test]
    fn test_message_format_json() {
        use crate::cdc::event::{Operation, SourceInfo};

        let event = ChangeEvent::new(
            SourceInfo::postgres("testdb", "public", "server"),
            Operation::Insert,
            "users",
            "user_1",
        );

        let format = MessageFormat::Json;
        let bytes = format.serialize(&event).unwrap();

        assert!(!bytes.is_empty());
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["operation"], "insert");
    }
}
